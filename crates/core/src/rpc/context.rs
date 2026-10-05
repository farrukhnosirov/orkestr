//! `context.*` handlers (§20, §68.40): what the Context Optimizer would give
//! an agent working on a task, with the reason and token count of every item.
//!
//! Both methods run the agent runtime's own assembly
//! ([`orkestr_agent_runtime::context_plan::plan_for_task`]) — previews never
//! call a model (handoff compression is deterministic here).

use orkestr_agent_runtime::context_plan::{plan_for_task, ContextOptions, PlannedContext, DEFAULT_MAX_FILES};
use orkestr_context::optimizer::SECTION_OVERHEAD;
use orkestr_context::{estimate_tokens, Decision, FileCandidate, NoteCandidate, Reason};
use orkestr_domain::TaskId;
use orkestr_protocol::methods::context::{
    self as m, ContextBudgetInfo, ContextDecision, ContextExplainResult, ContextIndexInfo, ContextItem,
    ContextItemKind, ContextLineRange, ContextPreviewResult, ContextReason,
};
use orkestr_protocol::RpcError;

use super::agents::agent_to_rpc;
use super::{RpcContext, RpcRouter};

/// Max files a preview may ask for.
pub const MAX_PREVIEW_FILES: u32 = 50;

/// Converts an optimizer reason (same JSON shape).
#[must_use]
pub fn reason(r: &Reason) -> ContextReason {
    match r {
        Reason::Mentioned => ContextReason::Mentioned,
        Reason::Focus => ContextReason::Focus,
        Reason::TermMatch { terms } => ContextReason::TermMatch { terms: terms.clone() },
        Reason::Defines { symbol } => ContextReason::Defines { symbol: symbol.clone() },
        Reason::ImportedBy { file } => ContextReason::ImportedBy { file: file.clone() },
        Reason::Imports { file } => ContextReason::Imports { file: file.clone() },
        Reason::References { symbol, file } => ContextReason::References { symbol: symbol.clone(), file: file.clone() },
        Reason::RecentChange { commits } => ContextReason::RecentChange { commits: *commits },
        Reason::Rule { origin, overrides_global } => {
            ContextReason::Rule { origin: origin.clone(), overrides_global: *overrides_global }
        }
        Reason::Pinned => ContextReason::Pinned,
        Reason::Memory { relevance_pct, tags } => {
            ContextReason::Memory { relevance_pct: *relevance_pct, tags: tags.clone() }
        }
        Reason::Mistake => ContextReason::Mistake,
        Reason::KnownIssue => ContextReason::KnownIssue,
    }
}

/// Converts an optimizer decision.
#[must_use]
pub fn decision(d: Decision) -> ContextDecision {
    match d {
        Decision::Whole => ContextDecision::Whole,
        Decision::Excerpt => ContextDecision::Excerpt,
        Decision::Included => ContextDecision::Included,
        Decision::OmittedBudget => ContextDecision::OmittedBudget,
        Decision::OmittedUnreadable => ContextDecision::OmittedUnreadable,
        Decision::BelowCutoff => ContextDecision::BelowCutoff,
        Decision::Duplicate => ContextDecision::Duplicate,
    }
}

fn reasons(rs: &[Reason]) -> Vec<ContextReason> {
    rs.iter().map(reason).collect()
}

fn options(
    model: Option<orkestr_domain::ModelRef>,
    token_budget: Option<u32>,
    max_files: Option<u32>,
) -> Result<ContextOptions, RpcError> {
    if let Some(b) = token_budget {
        if !(256..=1_000_000).contains(&b) {
            return Err(RpcError::bad_request("tokenBudget must be between 256 and 1000000"));
        }
    }
    let max_files = max_files.map_or(DEFAULT_MAX_FILES, |n| n.clamp(1, MAX_PREVIEW_FILES) as usize);
    Ok(ContextOptions { model, token_budget, max_files, summarize: false, ..ContextOptions::default() })
}

fn task_item(p: &PlannedContext) -> ContextItem {
    ContextItem {
        kind: ContextItemKind::Task,
        source: "task".into(),
        title: p.plan.bundle.task.lines().next().unwrap_or_default().chars().take(120).collect(),
        memory_kind: None,
        tokens: estimate_tokens(&p.plan.bundle.task) + SECTION_OVERHEAD,
        decision: ContextDecision::Included,
        reasons: Vec::new(),
        score: None,
        base_score: None,
        graph_score: None,
        churn_boost: None,
        ranges: Vec::new(),
    }
}

fn handoff_item(p: &PlannedContext) -> Option<ContextItem> {
    let text = p.plan.bundle.handoff.as_ref()?;
    Some(ContextItem {
        kind: ContextItemKind::Handoff,
        source: "handoff".into(),
        title: p.handoff.as_ref().map_or_else(String::new, |h| format!("{} entries", h.original_entries)),
        memory_kind: None,
        tokens: estimate_tokens(text) + SECTION_OVERHEAD,
        decision: ContextDecision::Included,
        reasons: Vec::new(),
        score: None,
        base_score: None,
        graph_score: None,
        churn_boost: None,
        ranges: Vec::new(),
    })
}

fn note_item(n: &NoteCandidate) -> ContextItem {
    ContextItem {
        kind: if n.source.starts_with("rule:") { ContextItemKind::Rule } else { ContextItemKind::Memory },
        source: n.source.clone(),
        title: n.title.clone(),
        memory_kind: Some(n.kind),
        tokens: n.tokens,
        decision: decision(n.decision),
        reasons: reasons(&n.reasons),
        score: Some(n.score),
        base_score: None,
        graph_score: None,
        churn_boost: None,
        ranges: Vec::new(),
    }
}

fn file_item(f: &FileCandidate, p: &PlannedContext) -> ContextItem {
    let ranges = p
        .plan
        .bundle
        .files
        .iter()
        .find(|b| b.path == f.path)
        .map(|b| {
            b.excerpts.iter().map(|e| ContextLineRange { start_line: e.start_line, end_line: e.end_line }).collect()
        })
        .unwrap_or_default();
    ContextItem {
        kind: ContextItemKind::File,
        source: f.path.clone(),
        title: f.path.clone(),
        memory_kind: None,
        tokens: f.tokens,
        decision: decision(f.decision),
        reasons: reasons(&f.reasons),
        score: Some(f.score),
        base_score: Some(f.base_score),
        graph_score: Some(f.graph_score),
        churn_boost: Some(f.churn_boost),
        ranges,
    }
}

fn included(d: ContextDecision) -> bool {
    matches!(d, ContextDecision::Whole | ContextDecision::Excerpt | ContextDecision::Included)
}

/// All considered items: task, handoff, rules, memory, files (included first
/// within each group, in prompt order).
fn all_items(p: &PlannedContext) -> Vec<ContextItem> {
    let mut items = vec![task_item(p)];
    items.extend(handoff_item(p));
    let notes: Vec<ContextItem> = p.plan.notes.iter().map(note_item).collect();
    let (rules, memory): (Vec<ContextItem>, Vec<ContextItem>) =
        notes.into_iter().partition(|i| i.kind == ContextItemKind::Rule);
    for group in [rules, memory] {
        let (mut inc, rest): (Vec<_>, Vec<_>) = group.into_iter().partition(|i| included(i.decision));
        inc.extend(rest);
        items.extend(inc);
    }
    items.extend(p.plan.files.iter().map(|f| file_item(f, p)));
    items
}

fn budget_info(p: &PlannedContext) -> ContextBudgetInfo {
    ContextBudgetInfo {
        model: p.model.clone(),
        context_window: p.context_window,
        reserved_output: if p.context_window.is_some() { p.budget.reserved_output() } else { 0 },
        system_tokens: p.budget.system_tokens,
        cap: p.budget.cap,
        budget: p.effective_budget,
        used: p.plan.bundle.total_tokens,
    }
}

/// `context.preview` result of a plan.
#[must_use]
pub fn preview_result(task_id: TaskId, p: &PlannedContext) -> ContextPreviewResult {
    let all = all_items(p);
    let omitted = u32::try_from(all.iter().filter(|i| !included(i.decision)).count()).unwrap_or(u32::MAX);
    let items: Vec<ContextItem> = all.into_iter().filter(|i| included(i.decision)).collect();
    ContextPreviewResult {
        task_id,
        items,
        omitted,
        budget: budget_info(p),
        index: ContextIndexInfo {
            files: p.index.files,
            import_edges: p.index.import_edges,
            churn_commits: p.index.churn_commits,
            truncated: p.index.truncated,
            has_root: p.index.has_root,
        },
        handoff_compression: p.handoff.as_ref().map(|h| format!("{:?}", h.method)),
        rendered: p.plan.bundle.render(),
    }
}

/// `context.explain` result of a plan.
#[must_use]
pub fn explain_result(task_id: TaskId, p: &PlannedContext, source: Option<&str>) -> ContextExplainResult {
    let wanted = source.map(|s| s.trim().replace('\\', "/")).filter(|s| !s.is_empty());
    let items =
        all_items(p).into_iter().filter(|i| wanted.as_deref().is_none_or(|w| i.source == w || i.title == w)).collect();
    ContextExplainResult { task_id, items, budget: budget_info(p) }
}

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::Preview, _, _>(|ctx: RpcContext, p| async move {
            let opts = options(p.model, p.token_budget, p.max_files)?;
            let plan = plan_for_task(&ctx.services.agents, p.task_id, &opts).await.map_err(agent_to_rpc)?;
            Ok(preview_result(p.task_id, &plan))
        })
        .method::<m::Explain, _, _>(|ctx: RpcContext, p| async move {
            let opts = options(p.model, p.token_budget, p.max_files)?;
            let plan = plan_for_task(&ctx.services.agents, p.task_id, &opts).await.map_err(agent_to_rpc)?;
            Ok(explain_result(p.task_id, &plan, p.source.as_deref()))
        });
}
