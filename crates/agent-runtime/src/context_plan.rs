//! Context assembly for a task (§20 Context Optimizer, §57 handoff, §68.40).
//!
//! Gathers every input of [`orkestr_context::optimize`] from the engines and
//! runs it — the same code path for an agent's initial message and for the
//! `context.preview` / `context.explain` RPCs, so the preview shows exactly
//! what an agent receives:
//!
//! * **Rules** — effective rules (project > global by key; the overridden
//!   global rule is passed too so the explanation says "overrides global").
//! * **Memory** — FTS relevance of the task text (normalized to 0..1), a
//!   dedicated search over `Mistake` / `KnownIssue` items, and the project's
//!   recorded mistakes / known issues (tag matches lift them).
//! * **Files** — the cached [`orkestr_context::global_cache`] index + import
//!   graph of the agent's workspace (invalidated by `FileChanged`), churn from
//!   `git log`, files the task already touched as focus paths.
//! * **Budget** — model-aware: the model's context window (provider
//!   descriptor / model list) minus reserved output and the system prompt,
//!   capped by the runtime's context budget.
//! * **Knowledge** — Knowledge Base chunks matching the task (indexed docs,
//!   FTS relevance), rendered after the bundle within a share of the budget.
//! * **Handoff** — every handoff of the task, compressed: by the model
//!   configured for the `Summarize` routing purpose when there is one
//!   (bounded by a timeout, usage recorded), else deterministically.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use orkestr_context::handoff::SUMMARIZE_INSTRUCTION;
use orkestr_context::{
    compress_history, global_cache, optimize, ChurnStats, CompressedHistory, ContextPlan, FsReader, HistoryEntry,
    MemoryInput, ModelBudget, OptimizeRequest, RuleInput, Signals, Summarizer,
};
use orkestr_domain::{
    now, MemoryAccess, MemoryItem, MemoryKind, MemoryScope, ModelRef, Project, Task, TaskId, UsageRecord, UsageRecordId,
};
use orkestr_protocol::methods::memory::{KnowledgeHit, ListMemoryParams, SearchKnowledgeParams, SearchMemoryParams};
use orkestr_providers::{ChatEvent, ChatMessage, ChatRequest, HANDOFF_SUMMARY_MARKER};
use tokio_util::sync::CancellationToken;

use crate::deps::RuntimeDeps;
use crate::error::{AgentError, AgentResult};
use crate::store::AgentStore;
use crate::AgentRuntime;

/// Routing purpose of the (cheap) handoff summarizer model.
pub const SUMMARIZE_PURPOSE: &str = "Summarize";
/// Estimated system prompt + tool schema tokens when the caller does not know.
pub const DEFAULT_SYSTEM_TOKENS: u32 = 2_500;
/// Default max files.
pub const DEFAULT_MAX_FILES: usize = 8;
/// Max handoffs of a task fed into compression.
pub const MAX_HANDOFFS: u32 = 20;
/// Git history window for churn.
pub const CHURN_SINCE: &str = "--since=120.days";
/// Max commits read for churn.
pub const CHURN_MAX_COMMITS: &str = "-n400";
/// Timeout of one summarizer call.
pub const SUMMARIZE_TIMEOUT: Duration = Duration::from_secs(45);
/// Memory hits considered.
const MEMORY_HITS: u32 = 12;
/// Relevance of a recorded mistake / known issue the task text did not match.
const UNMATCHED_ISSUE_RELEVANCE: f64 = 0.15;

/// Options of a context plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextOptions {
    /// Model to budget for (default: the task's model).
    pub model: Option<ModelRef>,
    /// Bundle cap (default: `RuntimeConfig::context_token_budget`).
    pub token_budget: Option<u32>,
    /// Max files.
    pub max_files: usize,
    /// Estimated system prompt tokens.
    pub system_tokens: u32,
    /// Allow a summarizer model call for long handoff histories (agent runs
    /// do; previews never spend tokens).
    pub summarize: bool,
}

impl Default for ContextOptions {
    fn default() -> Self {
        Self {
            model: None,
            token_budget: None,
            max_files: DEFAULT_MAX_FILES,
            system_tokens: DEFAULT_SYSTEM_TOKENS,
            summarize: false,
        }
    }
}

/// Facts about the indexed workspace.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IndexFacts {
    /// Indexed files.
    pub files: u32,
    /// Import edges.
    pub import_edges: u32,
    /// Commits in the churn window.
    pub churn_commits: u32,
    /// The index hit its file limit.
    pub truncated: bool,
    /// A workspace root exists.
    pub has_root: bool,
}

/// A planned context with everything needed to explain it.
#[derive(Debug, Clone)]
pub struct PlannedContext {
    /// The task header of the initial message.
    pub task_header: String,
    /// The optimizer plan (bundle + per-item decisions).
    pub plan: ContextPlan,
    /// Budget inputs.
    pub budget: ModelBudget,
    /// Bundle budget actually used.
    pub effective_budget: u32,
    /// Model the budget was computed for.
    pub model: Option<ModelRef>,
    /// Its context window, when known.
    pub context_window: Option<u32>,
    /// Workspace facts.
    pub index: IndexFacts,
    /// Handoff compression, when the task has a history.
    pub handoff: Option<CompressedHistory>,
    /// Knowledge Base chunks relevant to the task (indexed docs the agent
    /// can cite), best first.
    pub knowledge: Vec<KnowledgeHit>,
    /// `knowledge` rendered for the initial message (empty when none),
    /// within its share of the context budget.
    pub knowledge_text: String,
}

/// Knowledge Base hits considered for a task.
const KNOWLEDGE_HITS: u32 = 6;

/// Relevant Knowledge Base chunks of the task (project sources, plus the
/// global ones unless the template's memory access is project-only).
async fn knowledge(deps: &RuntimeDeps, access: MemoryAccess, project: &Project, task: &Task) -> Vec<KnowledgeHit> {
    let params = SearchKnowledgeParams {
        query: query_text(task),
        project_id: Some(project.id),
        include_global: Some(access != MemoryAccess::ProjectOnly),
        limit: Some(KNOWLEDGE_HITS),
    };
    match deps.memory.search_knowledge(params).await {
        Ok(hits) => hits,
        Err(e) => {
            tracing::warn!(error = %e, "context: knowledge search failed");
            Vec::new()
        }
    }
}

/// Renders knowledge hits within `max_chars` (whole entries only).
#[must_use]
pub fn render_knowledge(hits: &[KnowledgeHit], max_chars: usize) -> String {
    let mut out = String::new();
    for h in hits {
        let snippet: String = h.snippet.chars().filter(|c| *c != '\u{2}' && *c != '\u{3}').collect();
        let snippet = snippet.split_whitespace().collect::<Vec<_>>().join(" ");
        if snippet.is_empty() {
            continue;
        }
        let place = if h.heading_path.trim().is_empty() {
            h.path.clone()
        } else {
            format!("{} › {}", h.path, h.heading_path)
        };
        let line = format!("- **{}** — {place}: {snippet}\n", h.source_name);
        if out.len() + line.len() > max_chars {
            break;
        }
        out.push_str(&line);
    }
    if out.is_empty() {
        return out;
    }
    format!(
        "\n## Project knowledge (indexed docs)\nExcerpts of the project's Knowledge Base that match this task — follow them and cite the source when you rely on one.\n{out}"
    )
}

/// Task header of the initial message (title, description, criteria).
#[must_use]
pub fn task_header(task: &Task) -> String {
    let mut text = format!("# Task: {}\n", task.title);
    if !task.description.trim().is_empty() {
        text.push_str(&format!("\n{}\n", task.description.trim()));
    }
    if !task.acceptance_criteria.is_empty() {
        text.push_str("\nAcceptance criteria:\n");
        for c in &task.acceptance_criteria {
            text.push_str(&format!("- [{}] {}\n", if c.met { "x" } else { " " }, c.text));
        }
    }
    text
}

fn query_text(task: &Task) -> String {
    let mut q = format!("{}\n{}", task.title, task.description);
    for c in &task.acceptance_criteria {
        q.push('\n');
        q.push_str(&c.text);
    }
    q
}

/// Whether the configured summarizer model may receive this project's text:
/// the provider is known and enabled, is not a CLI coding agent, and the
/// project's AI privacy policy allows it.
fn summarizer_allowed(project: &Project, model: &ModelRef, info: Option<&crate::gateway::ProviderInfo>) -> bool {
    info.is_some_and(|i| {
        i.enabled && !i.is_cli_agent() && project.privacy.allows_provider(model.provider_id, i.is_local)
    })
}

/// A summarizer over the model gateway (the model configured for
/// [`SUMMARIZE_PURPOSE`]); usage is recorded against the task.
struct GatewaySummarizer<'a> {
    deps: &'a RuntimeDeps,
    model: ModelRef,
    task: &'a Task,
}

#[async_trait]
impl Summarizer for GatewaySummarizer<'_> {
    async fn summarize(&self, text: &str, max_tokens: u32) -> Result<String, String> {
        let mut req = ChatRequest::new(self.model.model.clone(), vec![ChatMessage::user(text.to_owned())]);
        req.system = Some(format!("{SUMMARIZE_INSTRUCTION}\n{HANDOFF_SUMMARY_MARKER}"));
        req.max_output_tokens = Some(max_tokens.max(32));
        let cancel = CancellationToken::new();
        let mut stream = self.deps.models.chat(vec![self.model.clone()], req, None, cancel.clone());
        let started = std::time::Instant::now();
        let collect = async {
            let mut out = String::new();
            let mut usage = None;
            while let Some(ev) = stream.next().await {
                match ev {
                    ChatEvent::TextDelta(t) => out.push_str(&t),
                    ChatEvent::Usage(u) => usage = Some(u),
                    ChatEvent::Done { .. } => break,
                    ChatEvent::Error(e) => return Err(e.to_string()),
                    _ => {}
                }
            }
            Ok((out, usage))
        };
        let result = tokio::time::timeout(SUMMARIZE_TIMEOUT, collect).await;
        let (text, usage) = match result {
            Ok(r) => r?,
            Err(_) => {
                cancel.cancel();
                return Err("summarizer timed out".into());
            }
        };
        if let Some(u) = usage {
            let priced = crate::gateway::call_cost(self.deps.models.as_ref(), &self.model, &u);
            let rec = UsageRecord {
                id: UsageRecordId::new(),
                ts: now(),
                provider_id: self.model.provider_id,
                model: self.model.model.clone(),
                project_id: Some(self.task.project_id),
                task_id: Some(self.task.id),
                agent_id: None,
                execution_id: None,
                input_tokens: u.input_tokens,
                output_tokens: u.output_tokens,
                cached_input_tokens: u.cached_input_tokens,
                reasoning_tokens: u.reasoning_tokens,
                cost_usd: priced.spend_usd,
                api_equivalent_usd: priced.api_equivalent_usd,
                estimated: u.estimated || u.reported_cost_usd.is_none(),
                latency_ms: Some(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)),
                success: true,
            };
            if let Err(e) = self.deps.cost.ingest(&rec).await {
                tracing::warn!(error = %e, "summarizer usage ingest failed");
            }
        }
        Ok(text)
    }
}

async fn rules(deps: &RuntimeDeps, project: &Project) -> Vec<RuleInput> {
    let effective = match deps.memory.effective_rules(Some(project.id), None).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "context: effective rules unavailable");
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    for r in effective {
        if let Some(global) = r.overrides {
            if let Ok(item) = deps.memory.get_item(global).await {
                out.push(RuleInput { item, key: Some(r.key.clone()) });
            }
        }
        out.push(RuleInput { item: r.item, key: Some(r.key) });
    }
    out
}

fn normalized(hits: Vec<(MemoryItem, f64)>) -> Vec<MemoryInput> {
    let max = hits.iter().map(|h| h.1).fold(0.0_f64, f64::max);
    hits.into_iter()
        .map(|(item, s)| MemoryInput { item, relevance: if max > 0.0 { (s / max).clamp(0.05, 1.0) } else { 0.5 } })
        .collect()
}

async fn memory(deps: &RuntimeDeps, access: MemoryAccess, project: &Project, task: &Task) -> Vec<MemoryInput> {
    let include_global = access != MemoryAccess::ProjectOnly;
    let query = query_text(task);
    let search = |kinds: Option<Vec<MemoryKind>>| SearchMemoryParams {
        query: query.clone(),
        project_id: Some(project.id),
        include_global: Some(include_global),
        kinds,
        limit: Some(MEMORY_HITS),
    };
    let mut out: Vec<MemoryInput> = Vec::new();
    match deps.memory.search(search(None)).await {
        Ok(hits) => out.extend(normalized(hits.into_iter().map(|h| (h.item, h.score)).collect())),
        Err(e) => tracing::warn!(error = %e, "context: memory search failed"),
    }
    let issues = Some(vec![MemoryKind::Mistake, MemoryKind::KnownIssue]);
    if let Ok(hits) = deps.memory.search(search(issues.clone())).await {
        out.extend(normalized(hits.into_iter().map(|h| (h.item, h.score)).collect()));
    }
    if let Ok(items) = deps
        .memory
        .list_items(ListMemoryParams {
            scope: MemoryScope::Project { project_id: project.id },
            kinds: issues,
            tag: None,
            pinned_only: None,
            limit: Some(20),
        })
        .await
    {
        out.extend(items.into_iter().map(|item| MemoryInput { item, relevance: UNMATCHED_ISSUE_RELEVANCE }));
    }
    out
}

async fn churn(deps: &RuntimeDeps, root: &Path) -> Option<ChurnStats> {
    let git = deps.vcs.git();
    if !git.is_available() {
        return None;
    }
    let out = git
        .runner()
        .run(
            root,
            ["log", "--no-merges", "--no-renames", "--name-only", "--format=%x1e%ct", CHURN_SINCE, CHURN_MAX_COMMITS],
            orkestr_vcs::RunOptions::read(),
        )
        .await
        .ok()?;
    out.success().then(|| ChurnStats::parse(&out.text()))
}

async fn handoff_history(deps: &RuntimeDeps, task: &Task, fallback: Option<String>) -> Vec<HistoryEntry> {
    let store = AgentStore::new(deps.db.clone());
    let mut handoffs = store.task_handoffs(task.id, MAX_HANDOFFS).await.unwrap_or_default();
    handoffs.reverse(); // oldest first
    let mut entries: Vec<HistoryEntry> =
        handoffs.iter().map(|h| HistoryEntry::new("handoff", crate::handoff::render(h))).collect();
    if entries.is_empty() {
        if let Some(text) = fallback.filter(|t| !t.trim().is_empty()) {
            entries.push(HistoryEntry::new("handoff", text));
        }
    }
    entries
}

/// Plans the context of `task` in workspace `root`.
#[allow(clippy::too_many_arguments)]
pub async fn plan_context(
    deps: &RuntimeDeps,
    access: MemoryAccess,
    project: &Project,
    task: &Task,
    root: Option<&Path>,
    fallback_handoff: Option<String>,
    opts: &ContextOptions,
) -> PlannedContext {
    let model = opts.model.clone().or_else(|| task.model.clone());
    let context_window = model.as_ref().and_then(|m| deps.models.context_window(m));
    let cap = opts.token_budget.unwrap_or(deps.config.context_token_budget).clamp(256, 1_000_000);
    let budget = ModelBudget {
        context_window: context_window.unwrap_or(0),
        reserve_output: deps.config.max_output_tokens.min(context_window.unwrap_or(u32::MAX) / 2),
        system_tokens: opts.system_tokens,
        cap,
    };
    let effective_budget = if context_window.is_some() { budget.effective() } else { cap };

    let (rules, memory) = if access == MemoryAccess::None {
        (Vec::new(), Vec::new())
    } else {
        (rules(deps, project).await, memory(deps, access, project, task).await)
    };
    let knowledge =
        if access == MemoryAccess::None { Vec::new() } else { knowledge(deps, access, project, task).await };
    // About an eighth of the bundle budget (≈ 4 chars per token).
    let knowledge_text = render_knowledge(&knowledge, (effective_budget as usize / 8).clamp(400, 8_000) * 4);

    let history = handoff_history(deps, task, fallback_handoff).await;
    let handoff = if history.is_empty() {
        None
    } else {
        let max = (effective_budget / 5).max(64);
        let configured = if opts.summarize { deps.models.configured_model(SUMMARIZE_PURPOSE).await } else { None };
        // The routing rule's model is global; the project's AI privacy policy
        // (local-only, provider allowlist) applies to it like to every other
        // chat path — otherwise deterministic compression is used.
        let configured =
            configured.filter(|m| summarizer_allowed(project, m, deps.models.provider(m.provider_id).as_ref()));
        let summarizer = configured.map(|model| GatewaySummarizer { deps, model, task });
        Some(compress_history(&history, max, summarizer.as_ref().map(|s| s as &dyn Summarizer)).await)
    };

    let mut index = IndexFacts { has_root: root.is_some(), ..IndexFacts::default() };
    let churn = match root {
        Some(r) => churn(deps, r).await,
        None => None,
    };
    index.churn_commits = churn.as_ref().map_or(0, |c| c.commits);
    let req = OptimizeRequest {
        task: query_text(task),
        token_budget: effective_budget,
        rules,
        memory,
        handoff: handoff.as_ref().map(|h| h.text.clone()),
        max_files: opts.max_files.clamp(1, 50),
        focus_paths: task.files_touched.clone(),
    };
    let fallback_req = OptimizeRequest { focus_paths: Vec::new(), ..req.clone() };
    let plan = match root.map(Path::to_path_buf) {
        Some(root) => {
            let job = tokio::task::spawn_blocking(move || -> Option<(ContextPlan, IndexFacts)> {
                let project = match global_cache().get(&root) {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!(error = %e, root = %root.display(), "context: workspace index unavailable");
                        return None;
                    }
                };
                let facts = IndexFacts {
                    files: u32::try_from(project.index.files.len()).unwrap_or(u32::MAX),
                    import_edges: u32::try_from(project.graph.edge_count()).unwrap_or(u32::MAX),
                    truncated: project.index.truncated,
                    ..IndexFacts::default()
                };
                let signals = Signals { graph: Some(&project.graph), churn: churn.as_ref() };
                Some((optimize(&project.index, &FsReader::new(&root), &req, signals), facts))
            });
            match job.await {
                Ok(Some((plan, facts))) => {
                    index.files = facts.files;
                    index.import_edges = facts.import_edges;
                    index.truncated = facts.truncated;
                    Some(plan)
                }
                _ => None,
            }
        }
        None => None,
    };
    // No (readable) workspace: task, rules, memory and handoff only.
    let plan = plan.unwrap_or_else(|| {
        optimize(
            &orkestr_context::FileIndex::default(),
            &FsReader::new(std::env::temp_dir()),
            &fallback_req,
            Signals::default(),
        )
    });
    PlannedContext {
        task_header: task_header(task),
        plan,
        budget,
        effective_budget,
        model,
        context_window,
        index,
        handoff,
        knowledge,
        knowledge_text,
    }
}

/// The workspace an agent on `task` works in: its worktree when it still
/// exists, else the project root.
#[must_use]
pub fn workspace_root(project: &Project, task: &Task) -> Option<PathBuf> {
    if let Some(w) = task.worktree.as_ref().map(|w| PathBuf::from(&w.path)) {
        if w.is_dir() {
            return Some(w);
        }
    }
    project.root_path.as_ref().map(PathBuf::from).filter(|p| p.is_dir())
}

/// Plans the context an agent working on `task_id` would receive now.
pub async fn plan_for_task(
    runtime: &AgentRuntime,
    task_id: TaskId,
    opts: &ContextOptions,
) -> AgentResult<PlannedContext> {
    let deps: &Arc<RuntimeDeps> = runtime.deps();
    let task = deps.tasks.get(task_id).await?;
    let project = deps
        .projects
        .get(task.project_id)
        .await?
        .ok_or_else(|| AgentError::NotFound(format!("project {}", task.project_id)))?;
    let mut access = MemoryAccess::default();
    if let Some(agent_id) = task.agent_id {
        if let Ok(agent) = runtime.store().get(agent_id).await {
            if let Some(tid) = agent.template_id {
                if let Ok(t) = runtime.templates().get(tid).await {
                    access = t.memory_access;
                }
            }
        }
    }
    let root = workspace_root(&project, &task);
    Ok(plan_context(deps, access, &project, &task, root.as_deref(), None, opts).await)
}

#[cfg(test)]
mod tests {
    use orkestr_domain::{AiPolicy, ProviderId, ProviderKind};

    use super::*;
    use crate::gateway::ProviderInfo;

    fn info(kind: ProviderKind, is_local: bool, enabled: bool) -> ProviderInfo {
        ProviderInfo { kind, is_local, enabled }
    }

    #[test]
    fn summarizer_respects_the_project_privacy_policy() {
        let cloud = ModelRef { provider_id: ProviderId::new(), model: "gpt-4.1-mini".into() };
        let mut project = Project::new("Private", now());
        project.privacy.ai = AiPolicy::LocalModelsOnly;
        // A cloud Summarize routing rule must not see a local-only project's text.
        assert!(!summarizer_allowed(&project, &cloud, Some(&info(ProviderKind::OpenAi, false, true))));
        assert!(summarizer_allowed(&project, &cloud, Some(&info(ProviderKind::Ollama, true, true))));
        project.privacy.ai = AiPolicy::SelectedProvidersOnly { providers: vec![ProviderId::new()] };
        assert!(!summarizer_allowed(&project, &cloud, Some(&info(ProviderKind::OpenAi, false, true))));
        project.privacy.ai = AiPolicy::CloudAllowed;
        assert!(summarizer_allowed(&project, &cloud, Some(&info(ProviderKind::OpenAi, false, true))));
        // Disabled, unknown and CLI providers are never used for summaries.
        assert!(!summarizer_allowed(&project, &cloud, Some(&info(ProviderKind::OpenAi, false, false))));
        assert!(!summarizer_allowed(&project, &cloud, Some(&info(ProviderKind::ClaudeCode, true, true))));
        assert!(!summarizer_allowed(&project, &cloud, None));
    }
}
