//! Bug Detective (`detective.*`, ADR-0042). **Owner: WP6.**
//!
//! Handlers over [`orkestr_detective::Detective`] plus the composition of its
//! [`Ports`] from core services: providers (hypothesis), agent runtime (fix
//! agent on the `detective/<id>` worktree), quality executor (reproduction
//! and fix tests), reviews, VCS, and the monitor / deploy areas (read through
//! their frozen `monitor.*` / `deploy.*` RPC contracts, so WP4/WP5 internals
//! stay free to change). [`install`] runs at boot; every handler also calls
//! it lazily. Adapters hold a `Weak<Services>` (no reference cycle).

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use orkestr_detective::ports::{
    DeploymentsReader, FixAgent, FixOutcome, FixRequest, HypothesisModel, ModelReply, ModelRequest, MonitorReader,
    ProjectRoots, QualityFailure, QualityReader, ReviewOutcome, ReviewRequest, Reviewer, TestRequest, TestRunner,
};
use orkestr_detective::{DetectiveError, GitVcs, Ports};
use orkestr_domain::{
    now, AgentId, AgentState, AlertId, Deployment, DeploymentId, ModelRef, ProjectId, Timestamp, UsageRecord,
    UsageRecordId,
};
use orkestr_orchestrator::deps::ManagerHub;
use orkestr_protocol::event::LogLevel;
use orkestr_protocol::methods::agents::{AgentStopMode, NewTaskSpec, SpawnAgentParams};
use orkestr_protocol::methods::deploy as dm;
use orkestr_protocol::methods::detective::{
    self as m, IncidentActionParams, IncidentResult, IncidentRun, ListIncidentsResult,
};
use orkestr_protocol::methods::monitor::{self as mm, Alert, AlertState, LogLine, LogQuery, MetricSeries};
use orkestr_protocol::methods::quality::{CheckDef, CheckKind, CheckSource, CheckStatus, RunTarget, Toolchain};
use orkestr_protocol::methods::reviews::{RequestReviewParams, ReviewVerdict};
use orkestr_protocol::methods::Method;
use orkestr_protocol::RpcError;
use orkestr_providers::{ChatEvent, ChatMessage, ChatRequest, RequiredCapabilities, ResponseFormat, RouteRequest};
use tokio_util::sync::CancellationToken;

use super::{RpcContext, RpcRouter};
use crate::services::Services;

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::Open, _, _>(|ctx: RpcContext, p: m::OpenIncidentParams| async move {
            install(&ctx.services).await;
            let by = ctx.services.security.user();
            let incident = ctx.services.detective.open(p, by).await?;
            Ok(IncidentResult { incident })
        })
        .method::<m::List, _, _>(|ctx: RpcContext, p: m::ListIncidentsParams| async move {
            install(&ctx.services).await;
            Ok(ListIncidentsResult { incidents: ctx.services.detective.list(&p).await? })
        })
        .method::<m::Get, _, _>(|ctx: RpcContext, p: IncidentActionParams| async move {
            install(&ctx.services).await;
            Ok(IncidentResult { incident: ctx.services.detective.get(p.incident_id).await? })
        })
        .method::<m::ApproveFix, _, _>(|ctx: RpcContext, p: IncidentActionParams| async move {
            install(&ctx.services).await;
            let by = ctx.services.security.user();
            let incident = ctx.services.detective.approve_fix(p.incident_id, p.note, by).await?;
            Ok(IncidentResult { incident })
        })
        .method::<m::ApproveMerge, _, _>(|ctx: RpcContext, p: IncidentActionParams| async move {
            install(&ctx.services).await;
            let by = ctx.services.security.user();
            let incident = ctx.services.detective.approve_merge(p.incident_id, p.note, by).await?;
            Ok(IncidentResult { incident })
        })
        .method::<m::Cancel, _, _>(|ctx: RpcContext, p: IncidentActionParams| async move {
            install(&ctx.services).await;
            let by = ctx.services.security.user();
            let incident = ctx.services.detective.cancel(p.incident_id, p.note, by).await?;
            Ok(IncidentResult { incident })
        })
        .method::<m::Settings, _, _>(|ctx: RpcContext, _p: orkestr_protocol::methods::Empty| async move {
            Ok(ctx.services.detective.settings().await?)
        })
        .method::<m::SetSettings, _, _>(|ctx: RpcContext, p: m::DetectiveSettings| async move {
            install(&ctx.services).await;
            Ok(ctx.services.detective.set_settings(p).await?)
        });
}

/// Wires the detective's ports to core services (idempotent). Call once at
/// boot after `Arc<Services>` exists (recovers interrupted incidents and
/// starts the `AlertRaised` auto-open listener).
pub async fn install(services: &Arc<Services>) {
    if services.detective.is_installed() {
        return;
    }
    static ONCE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _g = ONCE.lock().await;
    if services.detective.is_installed() {
        return;
    }
    let core = Arc::new(CorePorts(Arc::downgrade(services)));
    services
        .detective
        .install(Ports {
            projects: core.clone(),
            vcs: Arc::new(GitVcs::new(services.vcs.clone())),
            model: core.clone(),
            agent: core.clone(),
            tests: core.clone(),
            reviewer: core.clone(),
            monitor: Some(core.clone()),
            deployments: Some(core.clone()),
            quality: Some(core),
        })
        .await;
}

fn internal(e: impl std::fmt::Display) -> DetectiveError {
    DetectiveError::Internal(e.to_string())
}

fn cap(s: &str, max: usize) -> String {
    orkestr_detective::collectors::cap(s, max)
}

/// Every port over core services.
struct CorePorts(Weak<Services>);

impl CorePorts {
    fn s(&self) -> Result<Arc<Services>, DetectiveError> {
        self.0.upgrade().ok_or_else(|| DetectiveError::Internal("core is shutting down".into()))
    }

    /// Calls a method of another area through the router (local principal).
    async fn call<M: Method>(&self, params: M::Params) -> Result<M::Result, DetectiveError> {
        static ROUTER: OnceLock<RpcRouter> = OnceLock::new();
        let router = ROUTER.get_or_init(super::build_router);
        let ctx = RpcContext {
            services: self.s()?,
            session_id: orkestr_domain::SessionId::new(),
            correlation_id: orkestr_events::current_correlation().unwrap_or_default(),
            principal: super::Principal::Local,
        };
        let params = serde_json::to_value(params).map_err(internal)?;
        let v = router
            .dispatch(ctx, M::NAME, params)
            .await
            .map_err(|e: RpcError| DetectiveError::Internal(format!("{}: {}", M::NAME, e.message)))?;
        serde_json::from_value(v).map_err(internal)
    }

    async fn project(&self, id: ProjectId) -> Result<orkestr_domain::Project, DetectiveError> {
        self.s()?
            .projects
            .get(id)
            .await
            .map_err(internal)?
            .ok_or_else(|| DetectiveError::NotFound(format!("project {id}")))
    }
}

#[async_trait]
impl ProjectRoots for CorePorts {
    async fn root(&self, project_id: ProjectId) -> Result<PathBuf, DetectiveError> {
        let p = self.project(project_id).await?;
        let root = p
            .root_path
            .map(PathBuf::from)
            .filter(|r| r.is_dir())
            .ok_or_else(|| DetectiveError::Invalid("the project has no folder on this machine".into()))?;
        Ok(root)
    }
}

#[async_trait]
impl MonitorReader for CorePorts {
    async fn alert(&self, id: AlertId) -> Result<Option<Alert>, DetectiveError> {
        let r = self
            .call::<mm::Alerts>(mm::AlertsParams { project_id: None, include_resolved: true, limit: Some(1000) })
            .await?;
        Ok(r.alerts.into_iter().find(|a| a.id == id))
    }
    async fn active_alerts(&self, project_id: ProjectId) -> Result<Vec<Alert>, DetectiveError> {
        let r = self
            .call::<mm::Alerts>(mm::AlertsParams {
                project_id: Some(project_id),
                include_resolved: false,
                limit: Some(20),
            })
            .await?;
        Ok(r.alerts.into_iter().filter(|a| a.state != AlertState::Resolved).collect())
    }
    async fn logs_around(
        &self,
        at: Timestamp,
        before: Duration,
        after: Duration,
        project_id: Option<ProjectId>,
        min_level: Option<LogLevel>,
        limit: u32,
    ) -> Result<Vec<LogLine>, DetectiveError> {
        let since = at - chrono::Duration::from_std(before).unwrap_or_else(|_| chrono::Duration::hours(1));
        let until = at + chrono::Duration::from_std(after).unwrap_or_else(|_| chrono::Duration::minutes(5));
        // Built from JSON so additive protocol fields never break this adapter.
        let q: LogQuery = serde_json::from_value(serde_json::json!({
            "projectId": project_id,
            "minLevel": min_level,
            "since": since,
            "limit": limit,
        }))
        .map_err(internal)?;
        let mut lines = self.call::<mm::Logs>(q).await?.lines;
        lines.retain(|l| l.ts <= until);
        Ok(lines)
    }
    async fn recent_metrics(&self, window: Duration) -> Result<Vec<MetricSeries>, DetectiveError> {
        let since = now() - chrono::Duration::from_std(window).unwrap_or_else(|_| chrono::Duration::minutes(15));
        let p: mm::MetricsParams = serde_json::from_value(serde_json::json!({ "since": since })).map_err(internal)?;
        Ok(self.call::<mm::Metrics>(p).await?.series)
    }
}

#[async_trait]
impl DeploymentsReader for CorePorts {
    async fn recent(&self, project_id: ProjectId, limit: u32) -> Result<Vec<Deployment>, DetectiveError> {
        let p: dm::ListDeploymentsParams =
            serde_json::from_value(serde_json::json!({ "projectId": project_id, "limit": limit })).map_err(internal)?;
        Ok(self.call::<dm::List>(p).await?.deployments)
    }
    async fn logs(&self, deployment_id: DeploymentId, max_lines: u32) -> Result<Vec<String>, DetectiveError> {
        let p: dm::DeployLogsParams =
            serde_json::from_value(serde_json::json!({ "deploymentId": deployment_id })).map_err(internal)?;
        let mut lines = self.call::<dm::Logs>(p).await?.lines;
        let keep = max_lines as usize;
        if lines.len() > keep {
            lines.drain(..lines.len() - keep);
        }
        Ok(lines)
    }
}

#[async_trait]
impl QualityReader for CorePorts {
    async fn failure(
        &self,
        project_id: ProjectId,
        run_id: Option<&str>,
    ) -> Result<Option<QualityFailure>, DetectiveError> {
        let s = self.s()?;
        let run = match run_id {
            Some(id) => Some(s.quality.run(id).await.map_err(internal)?),
            None => s
                .quality
                .runs(project_id, None, 20)
                .await
                .map_err(internal)?
                .into_iter()
                .find(|r| r.status == CheckStatus::Failed),
        };
        Ok(run.map(|r| {
            let failed: Vec<_> =
                r.results.iter().filter(|c| matches!(c.status, CheckStatus::Failed | CheckStatus::TimedOut)).collect();
            let mut out = String::new();
            for c in &failed {
                out.push_str(&format!("## {} (`{}`) вЂ” {:?}\n{}\n", c.label, c.command, c.status, c.summary));
                for f in &c.failures {
                    out.push_str(&format!("- {}: {}\n", f.name, f.message));
                }
                out.push_str(&c.raw_tail);
                out.push('\n');
            }
            QualityFailure {
                run_id: r.id,
                failed_checks: failed.iter().map(|c| c.label.clone()).collect(),
                output: out,
            }
        }))
    }
}

#[async_trait]
impl HypothesisModel for CorePorts {
    async fn complete(&self, req: ModelRequest, cancel: CancellationToken) -> Result<ModelReply, DetectiveError> {
        let s = self.s()?;
        let project = self.project(req.project_id).await?;
        let models = Arc::clone(&s.agents.deps().models);
        let privacy = project.effective_privacy();
        let mut chain: Vec<ModelRef> = Vec::new();
        if let Some(m) = s.providers.manager_config().await.ok().and_then(|c| c.model) {
            if models.provider(m.provider_id).is_some_and(|i| privacy.allows_provider(m.provider_id, i.is_local)) {
                chain.push(m);
            }
        }
        let routed = models
            .route(&RouteRequest {
                purpose: Some("Detective".into()),
                required: RequiredCapabilities::default(),
                preferred: chain.first().cloned(),
                fallback_chain: Vec::new(),
                policy: privacy.ai,
                max_cost_usd: None,
                est_input_tokens: 16_000,
                max_output_tokens: 4_096,
                exclude: Vec::new(),
            })
            .await
            .map_err(|e| DetectiveError::Invalid(format!("no model for the Bug Detective: {e}")))?;
        for m in routed {
            if !chain.contains(&m) {
                chain.push(m);
            }
        }
        chain.retain(|m| models.provider(m.provider_id).is_some_and(|i| i.enabled && i.chat_capable()));
        let head = chain
            .first()
            .cloned()
            .ok_or_else(|| DetectiveError::Invalid("no chat model is available (connect a provider)".into()))?;
        let structured = orkestr_orchestrator::RegistryHub(Arc::clone(&s.providers)).supports_structured_output(&head);
        let messages = req
            .messages
            .iter()
            .map(
                |(role, text)| if role == "assistant" { ChatMessage::assistant(text) } else { ChatMessage::user(text) },
            )
            .collect::<Vec<_>>();
        let prompt_chars: usize = req.messages.iter().map(|(_, t)| t.len()).sum();
        let mut chat = ChatRequest::new(head.model.clone(), messages);
        let mut system = req.system.clone();
        if structured {
            chat.response_format =
                ResponseFormat::JsonSchema { name: "detective_hypothesis".into(), schema: req.schema.clone() };
        } else {
            system.push_str(&format!("\nJSON schema of the answer:\n{}\n", req.schema));
        }
        chat.system = Some(system);
        chat.max_output_tokens = Some(4_096);
        let started = std::time::Instant::now();
        let mut stream = models.chat(chain, chat, None, cancel.child_token());
        let (mut text, mut usage, mut model) = (String::new(), None, head);
        let mut ended = false;
        while let Some(ev) = stream.next().await {
            match ev {
                ChatEvent::TextDelta(t) => text.push_str(&t),
                ChatEvent::Usage(u) => usage = Some(u),
                ChatEvent::Switched { to, .. } => {
                    text.clear();
                    model = to;
                }
                ChatEvent::Done { .. } => {
                    ended = true;
                    break;
                }
                ChatEvent::Error(e) => {
                    if cancel.is_cancelled() {
                        return Err(DetectiveError::Cancelled);
                    }
                    return Err(DetectiveError::Internal(format!("model call failed: {e}")));
                }
                ChatEvent::ToolCall { .. } => {}
            }
        }
        if !ended {
            return Err(if cancel.is_cancelled() {
                DetectiveError::Cancelled
            } else {
                DetectiveError::Internal("the model stream ended unexpectedly".into())
            });
        }
        let usage = usage.unwrap_or_else(|| orkestr_providers::Usage::estimate(prompt_chars, text.len()));
        let priced = orkestr_agent_runtime::call_cost(models.as_ref(), &model, &usage);
        let rec = UsageRecord {
            id: UsageRecordId::new(),
            ts: now(),
            provider_id: model.provider_id,
            model: model.model.clone(),
            project_id: Some(req.project_id),
            task_id: None,
            agent_id: None,
            execution_id: None,
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cached_input_tokens: usage.cached_input_tokens,
            reasoning_tokens: usage.reasoning_tokens,
            cost_usd: priced.spend_usd,
            api_equivalent_usd: priced.api_equivalent_usd,
            estimated: usage.estimated || usage.reported_cost_usd.is_none(),
            latency_ms: Some(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)),
            success: true,
        };
        if let Err(e) = s.cost.ingest(&rec).await {
            tracing::warn!(error = %e, "detective usage ingest failed");
        }
        Ok(ModelReply { text, cost_usd: priced.spend_usd })
    }
}

/// Built-in template the fix agent runs (Backend developer).
fn fix_template() -> Result<orkestr_domain::AgentTemplateId, DetectiveError> {
    let all = orkestr_agent_runtime::templates::builtin_templates().map_err(internal)?;
    all.iter()
        .find(|(_, d)| d.role == orkestr_domain::AgentRole::Backend)
        .or_else(|| all.first())
        .map(|(id, _)| *id)
        .ok_or_else(|| DetectiveError::Internal("no built-in agent template".into()))
}

#[async_trait]
impl FixAgent for CorePorts {
    async fn run(
        &self,
        req: FixRequest,
        on_agent: Arc<dyn Fn(AgentId) + Send + Sync>,
        cancel: CancellationToken,
    ) -> Result<FixOutcome, DetectiveError> {
        let s = self.s()?;
        let params = SpawnAgentParams {
            project_id: req.project_id,
            template_id: fix_template()?,
            task_id: None,
            new_task: Some(NewTaskSpec {
                title: req.title.clone(),
                description: Some(req.prompt.clone()),
                acceptance_criteria: Some(vec![
                    "The root cause in the hypothesis is fixed".into(),
                    "A test reproduces the bug and passes".into(),
                ]),
                priority: None,
            }),
            model: None,
            // Own worktree based on `detective/<id>`; its commits are
            // fast-forwarded onto the fix branch afterwards.
            worktree_policy: Some(orkestr_domain::WorktreePolicy::DedicatedWorktree),
            name: Some(format!("Bug Detective #{}", req.attempt)),
            max_turns: None,
            base_branch: Some(req.branch.clone()),
            approval_mode: None,
            budget: Some(orkestr_domain::Budget {
                max_cost_usd: Some(req.budget_usd),
                ..orkestr_domain::Budget::unrestricted(orkestr_domain::BudgetPeriod::PerTask)
            }),
        };
        let (agent, task) = s.agents.spawn(params).await.map_err(internal)?;
        on_agent(agent.id);
        // Wait for the run to end.
        loop {
            if cancel.is_cancelled() {
                let _ = s.agents.stop(agent.id, AgentStopMode::Cancel).await;
                return Err(DetectiveError::Cancelled);
            }
            if s.agents.is_finished(agent.id).await.unwrap_or(false) {
                break;
            }
            tokio::select! {
                () = tokio::time::sleep(Duration::from_millis(500)) => {}
                () = cancel.cancelled() => {}
            }
        }
        let done = s.agents.get(agent.id).await.map_err(internal)?;
        let cost = done.usage.cost_usd;
        if done.state != AgentState::Completed {
            return Err(DetectiveError::Invalid(format!(
                "the fix agent ended {:?}: {}",
                done.state,
                done.last_error.unwrap_or_default()
            )));
        }
        // Bring the agent's commits onto `detective/<id>`.
        if let Some(w) = &done.worktree {
            let wt = Path::new(&w.path);
            if wt.is_dir() {
                let git = s.vcs.git();
                if git.stage(wt, &[], true).await.is_ok() {
                    let _ = git
                        .commit(wt, &format!("fix(detective): {}", cap(&req.hypothesis.summary, 72)), false, &[])
                        .await;
                }
                let out = git
                    .runner()
                    .run(
                        &req.worktree,
                        ["merge", "--ff-only", "--end-of-options", w.branch.as_str()],
                        orkestr_vcs::RunOptions::write(),
                    )
                    .await
                    .map_err(internal)?;
                if !out.success() {
                    return Err(DetectiveError::Conflict(format!(
                        "could not fast-forward the fix branch: {}",
                        out.text()
                    )));
                }
            }
        }
        Ok(FixOutcome {
            agent_id: Some(agent.id),
            task_id: Some(task.id.to_string()),
            cost_usd: cost,
            summary: done.activity.unwrap_or_else(|| "fix written".into()),
        })
    }

    async fn stop(&self, agent_id: AgentId) {
        if let Ok(s) = self.s() {
            let _ = s.agents.stop(agent_id, AgentStopMode::Cancel).await;
        }
    }
}

#[async_trait]
impl TestRunner for CorePorts {
    async fn run(&self, req: TestRequest, cancel: CancellationToken) -> Result<IncidentRun, DetectiveError> {
        let s = self.s()?;
        let q = &s.quality;
        let profile = q.profile(req.project_id, Some(&req.dir)).await.map_err(internal)?;
        let mut checks = profile.checks;
        let (kinds, ids) = match &req.command {
            Some(cmd) => {
                checks.push(CheckDef {
                    id: "detective-reproduction".into(),
                    kind: CheckKind::Test,
                    toolchain: Toolchain::Custom,
                    label: "Bug Detective reproduction".into(),
                    command: cmd.clone(),
                    cwd: ".".into(),
                    enabled: true,
                    timeout_secs: Some(600),
                    source: CheckSource::User,
                });
                (Vec::new(), vec!["detective-reproduction".to_owned()])
            }
            None => (vec![CheckKind::Test], Vec::new()),
        };
        let root = s.projects.get(req.project_id).await.ok().flatten().and_then(|p| p.root_path);
        let target = if root.as_deref().is_some_and(|r| Path::new(r) == req.dir) {
            RunTarget::Project
        } else {
            RunTarget::Worktree { path: req.dir.to_string_lossy().into_owned() }
        };
        let run = q
            .start(orkestr_quality::StartRun {
                project_id: req.project_id,
                task_id: None,
                agent_id: None,
                target,
                workspace: req.dir.clone(),
                checks,
                kinds,
                check_ids: ids,
                budget: orkestr_quality::QualityService::default_budget(),
            })
            .await
            .map_err(internal)?;
        let done = tokio::select! {
            r = q.wait(&run.id) => r.map_err(internal)?,
            () = cancel.cancelled() => {
                let _ = q.cancel(&run.id);
                let _ = tokio::time::timeout(Duration::from_secs(10), q.wait(&run.id)).await;
                return Err(DetectiveError::Cancelled);
            }
        };
        let mut out = String::new();
        for c in &done.results {
            out.push_str(&format!("## {} (`{}`) вЂ” {:?}: {}\n", c.label, c.command, c.status, c.summary));
            for f in &c.failures {
                out.push_str(&format!("- {}: {}\n", f.name, f.message));
            }
            if c.status != CheckStatus::Passed {
                out.push_str(&c.raw_tail);
                out.push('\n');
            }
        }
        if done.results.is_empty() {
            out.push_str("no test checks are configured for this project");
        }
        let exit_code = done.results.iter().find(|c| c.status != CheckStatus::Passed).and_then(|c| c.exit_code);
        Ok(IncidentRun {
            command: req
                .command
                .unwrap_or_else(|| done.results.iter().map(|c| c.command.clone()).collect::<Vec<_>>().join(" && ")),
            exit_code: exit_code.or(Some(0)),
            passed: done.status == CheckStatus::Passed,
            output_tail: orkestr_detective::collectors::cap_tail(&orkestr_security::redact::redact(&out), 16 * 1024),
            finished_at: done.finished_at.unwrap_or_else(now),
        })
    }
}

#[async_trait]
impl Reviewer for CorePorts {
    async fn review(&self, req: ReviewRequest, cancel: CancellationToken) -> Result<ReviewOutcome, DetectiveError> {
        let s = self.s()?;
        let Some(task_id) = req.task_id.as_deref().and_then(|t| t.parse().ok()) else {
            return Err(DetectiveError::Invalid("the fix has no task to review".into()));
        };
        let p = RequestReviewParams {
            task_id,
            model: None,
            adversarial: Some(false),
            transition: Some(false),
            wait: Some(true),
        };
        let r = tokio::select! {
            r = s.manager.request_review(p) => r.map_err(internal)?,
            () = cancel.cancelled() => return Err(DetectiveError::Cancelled),
        };
        let review = r.review;
        Ok(ReviewOutcome {
            review_id: Some(review.id.to_string()),
            approved: review.verdict == Some(ReviewVerdict::Approve),
            summary: review.summary,
            cost_usd: review.usage.cost_usd,
        })
    }
}
