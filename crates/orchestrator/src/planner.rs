//! Planning (§1 steps 1–4, §9 lite): study the project (memory, file index,
//! git, open tasks) → ask the Manager model for a structured plan (provider
//! structured output when supported, robust JSON extraction otherwise, one
//! repair round on invalid output) → normalise, validate, estimate → store as
//! a `Proposed` plan and show it as a plan card. With the `FullAuto` approval
//! policy the plan is approved and run immediately.

use std::path::PathBuf;

use orkestr_context::{FileIndex, IndexOptions};
use orkestr_domain::{now, ApprovalMode, ConversationId, ManagerPlanId, Project, TaskStatus, UsageSummary};
use orkestr_protocol::methods::manager::{ManagerPlan, ManagerRequestOptions, PlanSource, PlanStatus, MAX_CHAT_TEXT};
use orkestr_protocol::methods::memory::SearchMemoryParams;
use orkestr_protocol::methods::tasks::ListTasksParams;
use orkestr_providers::{ChatMessage, ChatRequest, ResponseFormat};
use tokio_util::sync::CancellationToken;

use crate::error::{OrchResult, OrchestratorError};
use crate::plan::{
    estimate_costs, extract_json, max_parallelism, normalize, plan_schema, PLAN_ROLES, PLAN_SCHEMA_NAME,
};
use crate::service::{Inner, Manager, MessageExtras};
use crate::util::{cap, is_ru, manager_chain, one_line, record_usage, ru_plural, stream_chat};

/// The planning system prompt (`language`: the user's UI language).
fn system_prompt(structured: bool, extra: &str, language: &str) -> String {
    let mut s = String::from(
        "You are the planning module of the Orkestr AI Manager. Decompose the user's goal into a plan of tasks \
         for specialised AI agents. Each task is done by ONE agent working alone in its own git worktree, so make \
         tasks concrete, independently verifiable and small enough for one agent (≈ 5–40 tool turns).\n\n\
         Rules:\n\
         - Group tasks by epic and feature (§9: idea → epic → features → tasks with acceptance criteria).\n\
         - Every task has testable acceptance criteria, one role, a priority and the keys of the tasks it depends on.\n\
         - Only add a dependency when the task really needs the other task's output; independent tasks run in parallel.\n\
         - List the files each task will likely touch (relative paths) so parallel tasks do not collide.\n\
         - Estimate tokens per task (input + output) honestly.\n\
         - Prefer 2–8 tasks; never more than 40.\n",
    );
    s.push_str(&format!("- Roles: {}.\n", PLAN_ROLES.join(", ")));
    if structured {
        s.push_str("\nAnswer with the JSON document only.\n");
    } else {
        s.push_str(&format!(
            "\nAnswer with ONE JSON object only (no prose, no markdown) that matches this JSON schema:\n{}\n",
            plan_schema()
        ));
    }
    if !extra.trim().is_empty() {
        s.push_str(&format!("\nInstructions from the user's AI Manager settings:\n{}\n", extra.trim()));
    }
    s.push('\n');
    s.push_str(&orkestr_agent_runtime::reply_language_instruction(language));
    s.push_str("- Field names and the task keys stay in English.\n");
    s
}

impl Inner {
    /// Gathers planning context (memory, file index, git, open tasks).
    pub async fn study(&self, project: &Project, goal: &str) -> String {
        let mut out = format!("# Project\nName: {}\n", project.name);
        if !project.description.trim().is_empty() {
            out.push_str(&format!("Description: {}\n", one_line(&project.description, 500)));
        }
        // Memory: rules + relevant items.
        if let Ok(rules) = self.deps.memory.effective_rules(Some(project.id), None).await {
            if !rules.is_empty() {
                out.push_str("\n# Project rules\n");
                for r in rules.iter().take(15) {
                    out.push_str(&format!("- {}: {}\n", r.item.title, one_line(&r.item.content, 240)));
                }
            }
        }
        if let Ok(hits) = self
            .deps
            .memory
            .search(SearchMemoryParams {
                query: cap(goal, 300),
                project_id: Some(project.id),
                include_global: Some(true),
                kinds: None,
                limit: Some(8),
            })
            .await
        {
            if !hits.is_empty() {
                out.push_str("\n# Relevant memory\n");
                for h in hits {
                    out.push_str(&format!(
                        "- [{:?}] {}: {}\n",
                        h.item.kind,
                        h.item.title,
                        one_line(&h.item.content, 240)
                    ));
                }
            }
        }
        // File index + git.
        let root = project.root_path.as_ref().map(PathBuf::from).filter(|p| p.is_dir());
        match &root {
            Some(root) => {
                let r = root.clone();
                let max = self.deps.config.planning_files;
                let goal_text = goal.to_owned();
                let listing = tokio::task::spawn_blocking(move || {
                    let index = FileIndex::build(&r, IndexOptions::default()).unwrap_or_default();
                    let total = index.files.len();
                    let ranked = orkestr_context::rank::rank_files(&index, &goal_text, 12);
                    let mut s =
                        format!("\n# Files ({total} indexed{})\n", if index.truncated { ", truncated" } else { "" });
                    for f in index.files.iter().take(max) {
                        s.push_str(&format!("- {}\n", f.path));
                    }
                    if total > max {
                        s.push_str(&format!("- … {} more\n", total - max));
                    }
                    let relevant: Vec<String> = ranked.into_iter().filter(|r| r.score > 0.0).map(|r| r.path).collect();
                    if !relevant.is_empty() {
                        s.push_str(&format!("\nMost relevant to the goal: {}\n", relevant.join(", ")));
                    }
                    s
                })
                .await
                .unwrap_or_default();
                out.push_str(&listing);
                let git = self.deps.vcs.git();
                if git.is_available() {
                    if let Ok(st) = git.status(root).await {
                        if st.is_repo {
                            let changed = st.entries.len();
                            out.push_str(&format!(
                                "\n# Git\nBranch: {} · {changed} uncommitted change(s)\n",
                                st.branch.as_deref().unwrap_or("(detached)")
                            ));
                            if let Ok(log) = git.log(root, None, None, 0, 5).await {
                                for c in log.commits {
                                    out.push_str(&format!("- {} {}\n", c.short_id, one_line(&c.subject, 100)));
                                }
                            }
                        } else {
                            out.push_str("\n# Git\nThe folder is not a git repository.\n");
                        }
                    }
                }
            }
            None => out.push_str("\nThe project has no root folder on this machine (no files to inspect).\n"),
        }
        // Open tasks (avoid duplicates).
        if let Ok(tasks) =
            self.deps.tasks.list(ListTasksParams { project_id: Some(project.id), ..ListTasksParams::default() }).await
        {
            let open: Vec<_> =
                tasks.iter().filter(|t| !matches!(t.status, TaskStatus::Completed | TaskStatus::Cancelled)).collect();
            if !open.is_empty() {
                out.push_str("\n# Open tasks (do not duplicate)\n");
                for t in open.iter().take(25) {
                    out.push_str(&format!("- [{:?}] {}\n", t.status, one_line(&t.title, 120)));
                }
            }
        }
        out
    }

    /// Produces, stores and publishes a `Proposed` plan.
    pub async fn make_plan(
        &self,
        project: &Project,
        goal: &str,
        options: ManagerRequestOptions,
        conversation: Option<ConversationId>,
        cancel: &CancellationToken,
    ) -> OrchResult<(ManagerPlan, UsageSummary)> {
        let goal = goal.trim();
        if goal.is_empty() || goal.chars().count() > MAX_CHAT_TEXT {
            return Err(OrchestratorError::Invalid(format!("the goal must be 1..={MAX_CHAT_TEXT} characters")));
        }
        if self.deps.security.emergency.flags().all_stopped {
            return Err(OrchestratorError::Forbidden(
                "Stop All AI is active — reset the emergency controls first".into(),
            ));
        }
        let cfg = self.config().await;
        if let Some(lang) = options.language.as_deref() {
            self.deps.config.ui_language.set(lang);
        }
        let language = self.deps.config.ui_language.get();
        let head = options.model.clone().or_else(|| cfg.model.clone());
        let structured = head.as_ref().is_some_and(|m| self.deps.hub.supports_structured_output(m));
        let chain = manager_chain(&self.deps, project, &cfg, options.model.as_ref(), true).await?;
        let structured = structured && chain.first() == head.as_ref();
        let context = self.study(project, goal).await;
        let user = format!("Goal: {goal}\n\n{}\n\nProduce the plan now.", cap(&context, 40_000));
        let mut messages = vec![ChatMessage::user(user)];
        let mut usage_total = UsageSummary::default();
        let mut last_err = String::new();
        for attempt in 0..2 {
            let mut req = ChatRequest::new(chain[0].model.clone(), messages.clone());
            req.system = Some(system_prompt(structured, &cfg.system_instructions, &language));
            req.response_format = if structured {
                ResponseFormat::JsonSchema { name: PLAN_SCHEMA_NAME.into(), schema: plan_schema() }
            } else {
                ResponseFormat::Text
            };
            req.max_output_tokens = Some(self.deps.config.max_output_tokens);
            req.reasoning = Some(cfg.reasoning);
            let out = stream_chat(&self.deps, chain.clone(), req, None, cancel).await?;
            let model = out.model.clone().unwrap_or_else(|| chain[0].clone());
            let used_model = Some(model.clone());
            let usage = out.usage.unwrap_or_else(|| {
                orkestr_providers::Usage::estimate(messages.iter().map(|m| m.content.len()).sum(), out.text.len())
            });
            usage_total.add(&record_usage(&self.deps, project.id, None, &model, usage, out.first_ms, true).await);
            let parsed = extract_json(&out.text)
                .ok_or_else(|| OrchestratorError::Planning("the answer contained no plan JSON object".into()))
                .and_then(|doc| normalize(&doc, goal));
            match parsed {
                Ok(draft) => {
                    let mut tasks = draft.tasks;
                    let estimated = estimate_costs(&mut tasks, used_model.as_ref());
                    let width = max_parallelism(&tasks);
                    let parallel = options.max_parallel_agents.unwrap_or(cfg.max_parallel_agents).max(1);
                    let mut warnings = draft.warnings;
                    if attempt > 0 {
                        warnings.push("the first answer was invalid and was repaired".into());
                    }
                    if estimated.is_none() {
                        warnings.push(
                            "cost estimate unavailable: the model's price is unknown (local, subscription or mock)"
                                .into(),
                        );
                    }
                    let at = now();
                    let plan = ManagerPlan {
                        id: ManagerPlanId::new(),
                        project_id: project.id,
                        conversation_id: conversation,
                        goal: draft.goal,
                        summary: if draft.summary.is_empty() {
                            format!("Plan for: {}", cap(goal, 200))
                        } else {
                            draft.summary
                        },
                        assumptions: draft.assumptions,
                        risks: draft.risks,
                        tasks,
                        estimated_cost_usd: estimated,
                        max_parallelism: width,
                        parallelism: width.min(parallel).max(1),
                        status: PlanStatus::Proposed,
                        source: if structured { PlanSource::Structured } else { PlanSource::Extracted },
                        model: used_model,
                        warnings,
                        options,
                        run_id: None,
                        rejection_reason: None,
                        created_at: at,
                        updated_at: at,
                    };
                    self.save_plan(&plan).await?;
                    self.supersede_proposals(project.id, plan.id).await?;
                    tracing::info!(plan_id = %plan.id, tasks = plan.tasks.len(), structured, "manager plan proposed");
                    return Ok((plan, usage_total));
                }
                Err(e) => {
                    last_err = e.to_string();
                    tracing::warn!(attempt, error = %last_err, "invalid plan from the model");
                    messages.push(ChatMessage::assistant(cap(&out.text, 20_000)));
                    messages.push(ChatMessage::user(format!(
                        "That was not a valid plan: {last_err}. Reply with only the corrected JSON object."
                    )));
                }
            }
        }
        Err(OrchestratorError::Planning(format!("the model did not produce a valid plan: {last_err}")))
    }

    /// Plans and — with the `FullAuto` policy — approves and starts the run.
    pub async fn plan_and_maybe_run(
        self: &std::sync::Arc<Self>,
        project: &Project,
        goal: &str,
        options: ManagerRequestOptions,
        conversation: Option<ConversationId>,
        cancel: &CancellationToken,
    ) -> OrchResult<(ManagerPlan, UsageSummary)> {
        let (plan, usage) = self.make_plan(project, goal, options, conversation, cancel).await?;
        let cfg = self.config().await;
        if self.policy(&cfg) == ApprovalMode::FullAuto {
            let mgr = Manager { inner: std::sync::Arc::clone(self) };
            let text = if is_ru(&self.deps.config.ui_language.get()) {
                "Режим подтверждений — «Полный автомат»: план подтверждён и запущен автоматически."
            } else {
                "Approval policy is Full Auto — the plan is approved and started automatically."
            };
            self.system_event(project.id, text, MessageExtras { plan_id: Some(plan.id), ..MessageExtras::default() })
                .await;
            let r = mgr
                .approve_plan(orkestr_protocol::methods::manager::ApprovePlanParams {
                    plan_id: plan.id,
                    edits: None,
                    start_run: Some(true),
                })
                .await?;
            return Ok((r.plan, usage));
        }
        Ok((plan, usage))
    }
}

impl Manager {
    /// Studies the project and proposes a plan for `goal` (`manager.plan`).
    /// The goal and the plan card are added to the conversation.
    pub async fn plan(&self, p: orkestr_protocol::methods::manager::ManagerPlanParams) -> OrchResult<ManagerPlan> {
        let project = self.inner.project(p.project_id).await?;
        let (conv, _) = self.inner.store.active_conversation(project.id).await?;
        self.inner
            .add_message(
                project.id,
                Some(conv),
                orkestr_protocol::methods::manager::ManagerMessageRole::User,
                &format!("Plan: {}", p.goal.trim()),
                MessageExtras::default(),
            )
            .await?;
        // Stop All AI / Stop project cancel planning; while Stop All is
        // active planning refuses with a notice (below).
        let cancel =
            crate::util::emergency_token(&self.inner.deps, project.id).unwrap_or_else(|_| CancellationToken::new());
        match self.inner.plan_and_maybe_run(&project, &p.goal, p.options.unwrap_or_default(), Some(conv), &cancel).await
        {
            Ok((plan, usage)) => {
                self.inner
                    .add_message(
                        project.id,
                        Some(conv),
                        orkestr_protocol::methods::manager::ManagerMessageRole::Manager,
                        &if is_ru(&self.inner.deps.config.ui_language.get()) {
                            let n = plan.tasks.len();
                            format!(
                                "Я изучил проект и подготовил план: {n} {}. {}",
                                ru_plural(n, "задача", "задачи", "задач"),
                                one_line(&plan.summary, 400)
                            )
                        } else {
                            format!(
                                "I studied the project and drafted a plan with {} task(s): {}",
                                plan.tasks.len(),
                                one_line(&plan.summary, 400)
                            )
                        },
                        MessageExtras {
                            plan_id: Some(plan.id),
                            model: plan.model.clone(),
                            usage: Some(usage),
                            ..MessageExtras::default()
                        },
                    )
                    .await?;
                Ok(plan)
            }
            Err(e) => {
                let text = if is_ru(&self.inner.deps.config.ui_language.get()) {
                    format!("Не удалось составить план: {e}")
                } else {
                    format!("Planning failed: {e}")
                };
                self.inner.system_event(project.id, &text, MessageExtras::default()).await;
                Err(e)
            }
        }
    }
}
