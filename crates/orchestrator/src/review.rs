//! Automatic code review (§17, §68.12).
//!
//! A Reviewer model reads the task's change (the diff of the agent's branch
//! against its base, plus uncommitted work in its worktree), the acceptance
//! criteria, the agent's own summary and the project's rules, and returns a
//! structured [`Review`]: verdict, findings (severity, category, file, line,
//! suggestion, evidence) and an acceptance-criteria checklist. The review is
//! persisted, published (`ReviewUpdated`, `ReviewFinished`) and — for a task
//! in `Review` — applied through the task engine:
//! `Approve → Testing`, `RequestChanges → Running`, `Reject → Failed`.
//! When an adversarial review ran together with it ([`crate::adversarial`]),
//! unresolved critical adversarial findings turn an approval into
//! "changes requested".
//!
//! The quality pipeline reads the outcome through [`ReviewGateSource`]
//! (gates "reviewer approved" and "no critical security finding").
//!
//! Bounds (§17, §64): one review per task and kind at a time, one repair
//! round for invalid output, a per-call timeout, the project's emergency
//! token. Only visible structured output is stored — never hidden reasoning.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use orkestr_domain::{
    now, ModelRef, Project, ProjectId, ReviewId, ReviewState, Task, TaskActor, TaskId, TaskStatus, UsageSummary,
};
use orkestr_protocol::event::{EventKind, ReviewFinishedPayload, ReviewPayload};
use orkestr_protocol::methods::reviews::{
    AcCheck, FindingSeverity, FindingStatus, ListReviewsParams, ListReviewsResult, RequestReviewParams,
    ResolveFindingParams, Review, ReviewDiffStats, ReviewFinding, ReviewGate, ReviewKind, ReviewResult, ReviewStatus,
    ReviewVerdict, MAX_FINDINGS, MAX_FINDING_TEXT,
};
use orkestr_protocol::methods::tasks::TaskPatch;
use orkestr_protocol::methods::vcs::{DiffTarget, FileDiff};
use orkestr_protocol::NewEvent;
use orkestr_providers::adapters::mock_review::REVIEW_MARKER;
use orkestr_providers::{ChatMessage, ChatRequest, RequiredCapabilities, ResponseFormat, RouteRequest};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::error::{OrchResult, OrchestratorError};
use crate::review_store::ReviewStore;
use crate::service::{Inner, Manager, MessageExtras};
use crate::util::{cap, is_ru, one_line, record_usage, stream_chat};

/// Timeout of one model call of a review or consensus step.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(300);
/// Max characters of the diff given to a reviewer.
pub const MAX_DIFF_CHARS: usize = 60_000;
/// Max output tokens of a review / proposal / synthesis call.
pub const MAX_OUTPUT_TOKENS: u32 = 6_000;
/// Max characters of a task-transition reason.
const MAX_REASON: usize = 400;

// ---------------------------------------------------------------------------
// shared helpers (also used by `adversarial` and `consensus`)
// ---------------------------------------------------------------------------

/// Reviews in flight: `(task, adversarial?)`. Task ids are globally unique, so
/// a process-wide set is safe across Manager instances.
static IN_FLIGHT: std::sync::Mutex<BTreeSet<(TaskId, bool)>> = std::sync::Mutex::new(BTreeSet::new());

/// Releases an in-flight slot on drop.
#[derive(Debug)]
pub(crate) struct InFlight((TaskId, bool));

impl InFlight {
    pub(crate) fn acquire(task: TaskId, adversarial: bool) -> OrchResult<Self> {
        let mut set = IN_FLIGHT.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if !set.insert((task, adversarial)) {
            return Err(OrchestratorError::Conflict(if adversarial {
                "an adversarial review of this task is already running".into()
            } else {
                "a review of this task is already running".into()
            }));
        }
        Ok(Self((task, adversarial)))
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        IN_FLIGHT.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&self.0);
    }
}

/// Ordered usable chat models for a quality role (head first): the explicit
/// model, routing for `purpose`, then the AI Manager model and its fallbacks.
/// Explicit models must be allowed by the project's privacy policy; CLI
/// coding agents and `exclude`d models are left out.
pub(crate) async fn role_chain(
    inner: &Inner,
    project: &Project,
    purpose: &str,
    explicit: Option<&ModelRef>,
    exclude: &[ModelRef],
) -> OrchResult<Vec<ModelRef>> {
    let deps = &inner.deps;
    let mut chain: Vec<ModelRef> = Vec::new();
    if let Some(m) = explicit {
        let info = deps.models.provider(m.provider_id).ok_or_else(|| {
            OrchestratorError::Invalid(format!("provider {} of model `{}` is not configured", m.provider_id, m.model))
        })?;
        if !project.effective_privacy().allows_provider(m.provider_id, info.is_local) {
            return Err(OrchestratorError::Forbidden(format!(
                "model `{}` is not allowed by the project's AI privacy policy",
                m.model
            )));
        }
        if !info.chat_capable() {
            return Err(OrchestratorError::Invalid(format!(
                "model `{}` is a CLI coding agent; reviews need a chat model",
                m.model
            )));
        }
        chain.push(m.clone());
    }
    let cfg = inner.config().await;
    match deps
        .models
        .route(&RouteRequest {
            purpose: Some(purpose.to_owned()),
            required: RequiredCapabilities::default(),
            preferred: explicit.cloned(),
            fallback_chain: Vec::new(),
            policy: project.effective_privacy().ai,
            max_cost_usd: None,
            est_input_tokens: 16_000,
            max_output_tokens: u64::from(MAX_OUTPUT_TOKENS),
            exclude: exclude.to_vec(),
        })
        .await
    {
        Ok(routed) => {
            for m in routed {
                if !chain.contains(&m) {
                    chain.push(m);
                }
            }
        }
        Err(e) => tracing::debug!(purpose, error = %e, "routing found no model; using the AI Manager model"),
    }
    for m in cfg.model.iter().chain(cfg.fallback_chain.iter()) {
        let allowed = deps
            .models
            .provider(m.provider_id)
            .is_some_and(|i| project.effective_privacy().allows_provider(m.provider_id, i.is_local));
        if allowed && !chain.contains(m) {
            chain.push(m.clone());
        }
    }
    chain.retain(|m| {
        !exclude.contains(m) && deps.models.provider(m.provider_id).is_some_and(|i| i.enabled && i.chat_capable())
    });
    if chain.is_empty() {
        return Err(OrchestratorError::NoModel(format!(
            "no chat model is available for {purpose} — connect a provider or choose the AI Manager model in Settings (the offline Mock provider also works)"
        )));
    }
    Ok(chain)
}

/// Output of one structured model call.
#[derive(Debug)]
pub(crate) struct Structured {
    /// The parsed JSON object.
    pub doc: Value,
    /// Model that answered.
    pub model: ModelRef,
    /// Usage of all attempts.
    pub usage: UsageSummary,
}

/// One structured model call: provider-enforced JSON schema when supported,
/// otherwise JSON extraction from text; one repair round on invalid output;
/// bounded by [`CALL_TIMEOUT`]. `parse` validates the document.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn structured_call<T>(
    inner: &Inner,
    project: ProjectId,
    task: Option<TaskId>,
    chain: Vec<ModelRef>,
    system: String,
    user: String,
    schema_name: &str,
    schema: Value,
    parse: impl Fn(&Value) -> Result<T, String>,
    cancel: &CancellationToken,
) -> OrchResult<(T, Structured)> {
    let head = chain.first().cloned().ok_or_else(|| OrchestratorError::NoModel("empty model chain".into()))?;
    let structured = inner.deps.hub.supports_structured_output(&head);
    let mut messages = vec![ChatMessage::user(user)];
    let mut usage_total = UsageSummary::default();
    let mut last_err = String::new();
    for attempt in 0..2 {
        if cancel.is_cancelled() {
            return Err(OrchestratorError::Conflict("cancelled".into()));
        }
        let mut req = ChatRequest::new(head.model.clone(), messages.clone());
        req.system = Some(if structured {
            format!("{system}\nAnswer with the JSON document only.\n")
        } else {
            format!("{system}\nAnswer with ONE JSON object only (no prose, no markdown) matching this JSON schema:\n{schema}\n")
        });
        req.response_format = if structured {
            ResponseFormat::JsonSchema { name: schema_name.into(), schema: schema.clone() }
        } else {
            ResponseFormat::Text
        };
        req.max_output_tokens = Some(MAX_OUTPUT_TOKENS);
        let out = match tokio::time::timeout(CALL_TIMEOUT, stream_chat(&inner.deps, chain.clone(), req, None, cancel))
            .await
        {
            Ok(r) => r?,
            Err(_) => {
                return Err(OrchestratorError::Planning(format!(
                    "the model did not answer within {} s",
                    CALL_TIMEOUT.as_secs()
                )))
            }
        };
        let model = out.model.clone().unwrap_or_else(|| head.clone());
        let usage = out.usage.unwrap_or_else(|| {
            orkestr_providers::Usage::estimate(messages.iter().map(|m| m.content.len()).sum(), out.text.len())
        });
        usage_total.add(&record_usage(&inner.deps, project, task, &model, usage, out.first_ms, true).await);
        let parsed = extract_object(&out.text)
            .ok_or_else(|| "the answer contained no JSON object".to_owned())
            .and_then(|doc| parse(&doc).map(|t| (t, doc)));
        match parsed {
            Ok((value, doc)) => return Ok((value, Structured { doc, model, usage: usage_total })),
            Err(e) => {
                tracing::warn!(attempt, schema_name, error = %e, "invalid structured answer");
                last_err = e;
                messages.push(ChatMessage::assistant(cap(&out.text, 20_000)));
                messages.push(ChatMessage::user(format!(
                    "That answer was invalid: {last_err}. Reply with only the corrected JSON object."
                )));
            }
        }
    }
    Err(OrchestratorError::Planning(format!("the model did not produce a valid answer: {last_err}")))
}

/// The JSON object of a model answer: the whole text, a fenced block, or the
/// first balanced `{…}` that parses.
#[must_use]
pub fn extract_object(text: &str) -> Option<Value> {
    let trimmed = text.trim();
    if let Ok(v @ Value::Object(_)) = serde_json::from_str::<Value>(trimmed) {
        return Some(v);
    }
    let mut rest = trimmed;
    while let Some(open) = rest.find("```") {
        let after = &rest[open + 3..];
        let body_start = after.find('\n').map_or(0, |n| n + 1);
        let body = &after[body_start..];
        let Some(close) = body.find("```") else { break };
        if let Ok(v @ Value::Object(_)) = serde_json::from_str::<Value>(body[..close].trim()) {
            return Some(v);
        }
        rest = &body[close + 3..];
    }
    let mut from = 0;
    while let Some(off) = trimmed[from..].find('{') {
        let start = from + off;
        if let Some(end) = balanced_end(trimmed, start) {
            if let Ok(v @ Value::Object(_)) = serde_json::from_str::<Value>(&trimmed[start..end]) {
                return Some(v);
            }
        }
        from = start + 1;
    }
    None
}

fn balanced_end(s: &str, start: usize) -> Option<usize> {
    let (mut depth, mut in_str, mut esc) = (0usize, false, false);
    for (i, c) in s[start..].char_indices() {
        if in_str {
            match (esc, c) {
                (true, _) => esc = false,
                (false, '\\') => esc = true,
                (false, '"') => in_str = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(start + i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// First non-empty string among `keys`, capped.
pub(crate) fn text_of(v: &Value, keys: &[&str], max: usize) -> String {
    keys.iter()
        .filter_map(|k| v.get(*k))
        .find_map(|x| match x {
            Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_owned()),
            Value::Array(a) if !a.is_empty() => {
                Some(a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n").trim().to_owned())
            }
            _ => None,
        })
        .map(|s| cap(&crate::util::redact(&s), max))
        .unwrap_or_default()
}

/// String list (array of strings, or one string split by lines).
pub(crate) fn list_of(v: &Value, key: &str, max_items: usize) -> Vec<String> {
    match v.get(key) {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|x| x.as_str().map(str::trim).filter(|s| !s.is_empty()))
            .take(max_items)
            .map(|s| cap(&crate::util::redact(s), 500))
            .collect(),
        Some(Value::String(s)) => s
            .lines()
            .map(|l| l.trim().trim_start_matches(['-', '*']).trim())
            .filter(|l| !l.is_empty())
            .take(max_items)
            .map(|s| cap(&crate::util::redact(s), 500))
            .collect(),
        _ => Vec::new(),
    }
}

/// Parses a severity leniently.
#[must_use]
pub fn parse_severity(v: Option<&Value>) -> FindingSeverity {
    let s = v.and_then(Value::as_str).unwrap_or_default().trim().to_ascii_lowercase();
    match s.as_str() {
        "critical" | "blocker" | "severe" => FindingSeverity::Critical,
        "high" | "major" | "error" => FindingSeverity::High,
        "medium" | "moderate" | "warning" => FindingSeverity::Medium,
        "low" | "minor" | "nit" => FindingSeverity::Low,
        _ => FindingSeverity::Info,
    }
}

/// Parses a verdict leniently.
#[must_use]
pub fn parse_verdict(v: Option<&Value>) -> Option<ReviewVerdict> {
    let s = v?.as_str()?.trim().to_ascii_lowercase().replace(['-', ' '], "_");
    match s.as_str() {
        "approve" | "approved" | "accept" | "accepted" | "lgtm" | "pass" => Some(ReviewVerdict::Approve),
        "request_changes" | "changes_requested" | "changes" | "needs_work" | "revise" => {
            Some(ReviewVerdict::RequestChanges)
        }
        "reject" | "rejected" | "fail" => Some(ReviewVerdict::Reject),
        _ => None,
    }
}

/// Findings of a review document (`findings` array), with ids `f1…`.
pub(crate) fn parse_findings(doc: &Value) -> Vec<ReviewFinding> {
    let Some(items) = doc.get("findings").and_then(Value::as_array) else { return Vec::new() };
    items
        .iter()
        .filter(|f| f.is_object())
        .take(MAX_FINDINGS)
        .enumerate()
        .map(|(i, f)| {
            let line = f.get("line").and_then(|l| {
                l.as_u64()
                    .or_else(|| l.as_str().and_then(|s| s.trim().parse().ok()))
                    .and_then(|n| u32::try_from(n).ok())
            });
            let file = f
                .get("file")
                .and_then(Value::as_str)
                .map(|s| s.trim().trim_start_matches("b/").replace('\\', "/"))
                .filter(|s| !s.is_empty() && s != "null" && !s.contains(".."))
                .map(|s| cap(&s, 500));
            let mut title = text_of(f, &["title", "summary"], 300);
            if title.is_empty() {
                title = text_of(f, &["detail", "description"], 120);
            }
            ReviewFinding {
                id: format!("f{}", i + 1),
                severity: parse_severity(f.get("severity")),
                category: {
                    let c = text_of(f, &["category", "kind", "type"], 60).to_ascii_lowercase();
                    if c.is_empty() {
                        "general".into()
                    } else {
                        c
                    }
                },
                file,
                line: line.filter(|l| *l > 0),
                title,
                detail: text_of(f, &["detail", "description", "explanation"], MAX_FINDING_TEXT),
                suggestion: text_of(f, &["suggestion", "fix", "recommendation"], MAX_FINDING_TEXT),
                evidence: text_of(f, &["evidence", "proof", "quote"], MAX_FINDING_TEXT),
                status: FindingStatus::Open,
                response: None,
                resolution_note: None,
                resolved_at: None,
            }
        })
        .filter(|f| !f.title.is_empty())
        .collect()
}

/// Acceptance-criteria check aligned to the task's criteria (by text, then position).
pub(crate) fn align_ac(doc: &Value, task: &Task) -> Vec<AcCheck> {
    let items: Vec<&Value> = doc
        .get("acceptanceCriteria")
        .or_else(|| doc.get("acceptance_criteria"))
        .or_else(|| doc.get("acCheck"))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter(|x| x.is_object()).collect())
        .unwrap_or_default();
    let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    task.acceptance_criteria
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let want = norm(&c.text);
            let hit = items
                .iter()
                .find(|x| x.get("criterion").and_then(Value::as_str).is_some_and(|t| norm(t) == want))
                .or_else(|| items.get(i));
            match hit {
                Some(x) => AcCheck {
                    criterion: c.text.clone(),
                    met: x.get("met").and_then(Value::as_bool).unwrap_or(false),
                    evidence: text_of(x, &["evidence", "note"], 1_000),
                },
                None => {
                    AcCheck { criterion: c.text.clone(), met: false, evidence: "not assessed by the reviewer".into() }
                }
            }
        })
        .collect()
}

/// Review JSON schema (also used by the adversarial review).
#[must_use]
pub fn review_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["verdict", "summary", "findings", "acceptanceCriteria"],
        "properties": {
            "verdict": {"type": "string", "enum": ["approve", "request_changes", "reject"]},
            "summary": {"type": "string"},
            "findings": {"type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["severity", "category", "file", "line", "title", "detail", "suggestion", "evidence"],
                "properties": {
                    "severity": {"type": "string", "enum": ["critical", "high", "medium", "low", "info"]},
                    "category": {"type": "string"},
                    "file": {"type": ["string", "null"]},
                    "line": {"type": ["integer", "null"]},
                    "title": {"type": "string"},
                    "detail": {"type": "string"},
                    "suggestion": {"type": "string"},
                    "evidence": {"type": "string"}
                }
            }},
            "acceptanceCriteria": {"type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["criterion", "met", "evidence"],
                "properties": {
                    "criterion": {"type": "string"},
                    "met": {"type": "boolean"},
                    "evidence": {"type": "string"}
                }
            }}
        }
    })
}

/// Parsed reviewer answer.
#[derive(Debug, Clone)]
pub(crate) struct ParsedReview {
    pub verdict: ReviewVerdict,
    pub summary: String,
    pub findings: Vec<ReviewFinding>,
    pub ac_check: Vec<AcCheck>,
}

/// Validates a reviewer document. An approval with an unresolved critical
/// finding is turned into "changes requested" (never approve past a blocker).
pub(crate) fn parse_review(doc: &Value, task: &Task) -> Result<ParsedReview, String> {
    let verdict = parse_verdict(doc.get("verdict").or_else(|| doc.get("decision")))
        .ok_or_else(|| "`verdict` must be one of approve, request_changes, reject".to_owned())?;
    let findings = parse_findings(doc);
    let mut summary = text_of(doc, &["summary", "overview"], 4_000);
    let verdict = if verdict == ReviewVerdict::Approve && findings.iter().any(ReviewFinding::is_blocking) {
        summary.push_str("\n\n(Approval withdrawn: the review reports an unresolved critical finding.)");
        ReviewVerdict::RequestChanges
    } else {
        verdict
    };
    Ok(ParsedReview { verdict, summary: summary.trim().to_owned(), findings, ac_check: align_ac(doc, task) })
}

/// The reviewed change: diff text for the model + stats.
#[derive(Debug, Clone, Default)]
pub(crate) struct Change {
    pub text: String,
    pub stats: ReviewDiffStats,
}

/// Collects the task's change: the diff of its branch against the merge base
/// with the project's `HEAD` plus uncommitted work in its worktree; without a
/// worktree, the uncommitted changes of the project folder.
pub(crate) async fn gather_change(inner: &Inner, project: &Project, task: &Task) -> Change {
    let none =
        |why: &str| Change { text: String::new(), stats: ReviewDiffStats { source: why.into(), ..Default::default() } };
    let Some(root) = project.root_path.as_ref().map(PathBuf::from).filter(|p| p.is_dir()) else {
        return none("the project has no folder on this machine");
    };
    let git = inner.deps.vcs.git();
    if !git.is_available() {
        return none("git is not installed");
    }
    if !matches!(git.repo_info(&root).await, Ok(Some(_))) {
        return none("the project folder is not a git repository");
    }
    let req = |target: DiffTarget, untracked: bool| orkestr_vcs::DiffRequest {
        target,
        paths: Vec::new(),
        context_lines: 3,
        include_untracked: untracked,
        max_patch_bytes: 64 * 1024,
    };
    let mut files: Vec<FileDiff> = Vec::new();
    let source;
    if let Some(w) = &task.worktree {
        let wt = PathBuf::from(&w.path);
        let has_wt = wt.is_dir();
        let base = git
            .runner()
            .run_ok(&root, ["merge-base", "HEAD", w.branch.as_str()], orkestr_vcs::RunOptions::read())
            .await
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .filter(|s| !s.is_empty());
        if let Some(base) = &base {
            let (dir, to) = if has_wt { (wt.clone(), "HEAD".to_owned()) } else { (root.clone(), w.branch.clone()) };
            match git.diff(&dir, &req(DiffTarget::Range { from: base.clone(), to }, false)).await {
                Ok(d) => files.extend(d.files),
                Err(e) => tracing::warn!(task_id = %task.id, error = %e, "review: branch diff failed"),
            }
        }
        if has_wt {
            match git.diff(&wt, &req(DiffTarget::Head, true)).await {
                Ok(d) => files.extend(d.files),
                Err(e) => tracing::warn!(task_id = %task.id, error = %e, "review: worktree diff failed"),
            }
        }
        source = format!("{}…{}", base.as_deref().map_or("HEAD", |b| &b[..b.len().min(10)]), w.branch);
    } else {
        match git.diff(&root, &req(DiffTarget::Head, true)).await {
            Ok(d) => files.extend(d.files),
            Err(e) => tracing::warn!(task_id = %task.id, error = %e, "review: working-tree diff failed"),
        }
        source = "uncommitted changes in the project folder".to_owned();
    }
    render_change(&files, source)
}

/// Renders file diffs into one bounded text.
pub(crate) fn render_change(files: &[FileDiff], source: String) -> Change {
    let mut text = String::new();
    let mut truncated = false;
    let mut paths: BTreeSet<&str> = BTreeSet::new();
    let (mut add, mut del) = (0u32, 0u32);
    for f in files {
        paths.insert(&f.path);
        add += f.additions;
        del += f.deletions;
        let part = if f.binary {
            format!("Binary file {} changed\n", f.path)
        } else {
            let mut p = f.patch.clone();
            if !p.ends_with('\n') {
                p.push('\n');
            }
            if f.truncated {
                p.push_str("… (patch truncated)\n");
                truncated = true;
            }
            p
        };
        if text.chars().count() + part.chars().count() > MAX_DIFF_CHARS {
            let room = MAX_DIFF_CHARS.saturating_sub(text.chars().count());
            text.extend(part.chars().take(room));
            text.push_str("\n… (diff truncated)\n");
            truncated = true;
            break;
        }
        text.push_str(&part);
    }
    Change {
        text: crate::util::redact(&text),
        stats: ReviewDiffStats {
            files: u32::try_from(paths.len()).unwrap_or(u32::MAX),
            additions: add,
            deletions: del,
            truncated,
            source,
        },
    }
}

/// The task description block shared by the review prompts.
pub(crate) async fn task_brief(inner: &Inner, project: &Project, task: &Task, change: &Change) -> String {
    let mut s = format!("# Task: {}\n", one_line(&task.title, 200));
    if !task.description.trim().is_empty() {
        s.push_str(&format!("{}\n", cap(task.description.trim(), 4_000)));
    }
    s.push_str("\n## Acceptance criteria\n");
    if task.acceptance_criteria.is_empty() {
        s.push_str("(none given)\n");
    }
    for (i, c) in task.acceptance_criteria.iter().enumerate() {
        s.push_str(&format!("{}. {}\n", i + 1, one_line(&c.text, 400)));
    }
    if let Some(r) = &task.result {
        s.push_str(&format!("\n## Agent's own summary\n{}\n", cap(r.summary.trim(), 2_000)));
    }
    if let Ok(rules) = inner.deps.memory.effective_rules(Some(project.id), None).await {
        if !rules.is_empty() {
            s.push_str("\n## Project rules\n");
            for r in rules.iter().take(10) {
                s.push_str(&format!("- {}: {}\n", r.item.title, one_line(&r.item.content, 240)));
            }
        }
    }
    s.push_str(&format!(
        "\n## Diff ({}) — {} file(s), +{} −{}{}\n```diff\n{}```\n",
        change.stats.source,
        change.stats.files,
        change.stats.additions,
        change.stats.deletions,
        if change.stats.truncated { ", truncated" } else { "" },
        change.text
    ));
    s
}

fn review_system(language: &str) -> String {
    format!(
        "{REVIEW_MARKER}\nYou are the code reviewer of Orkestr, an AI development OS. Review the change of ONE task \
         that an AI agent made.\n\
         - Check correctness, security, error handling, tests, maintainability and whether the change does what \
         the task asks.\n\
         - Check every acceptance criterion against the diff and say whether it is met, with evidence (file, test, line).\n\
         - Report only problems you can point to in the diff. Give the file (relative path) and the line in the NEW \
         version of the file when possible, a concrete suggestion and evidence (quote the code).\n\
         - Severity: critical = must not ship (security hole, data loss, broken build); high = serious bug; medium = \
         should fix; low = minor; info = note.\n\
         - Verdict: approve when the change can go to testing (no critical or high findings and the criteria are met), \
         request_changes when findings must be fixed first, reject when the approach is fundamentally wrong.\n\
         - Write a short summary of your assessment (no hidden reasoning, no chain-of-thought).\n\n{}\
         - JSON field names and enum values stay in English.\n",
        orkestr_agent_runtime::reply_language_instruction(language)
    )
}

// ---------------------------------------------------------------------------
// gate
// ---------------------------------------------------------------------------

/// What the quality pipeline reads (§17 gates "reviewer approved" and
/// "no critical security finding").
#[async_trait]
pub trait ReviewGateSource: Send + Sync {
    /// Gate view of a task's latest reviews.
    async fn review_gate(&self, task: TaskId) -> OrchResult<ReviewGate>;
}

#[async_trait]
impl ReviewGateSource for Manager {
    async fn review_gate(&self, task: TaskId) -> OrchResult<ReviewGate> {
        Manager::review_gate(self, task).await
    }
}

/// Gate of a task from its reviews (newest first).
#[must_use]
pub fn gate_of(task: TaskId, reviews: &[Review]) -> ReviewGate {
    let latest = |kind: ReviewKind| reviews.iter().find(|r| r.kind == kind && r.status == ReviewStatus::Finished);
    let std = latest(ReviewKind::Standard);
    let adv = latest(ReviewKind::Adversarial);
    let reviewer_approved = std.is_some_and(|r| r.verdict == Some(ReviewVerdict::Approve));
    let blocking: Vec<&ReviewFinding> = std.iter().chain(adv.iter()).flat_map(|r| r.blocking()).collect();
    let no_critical_security = !blocking.iter().any(|f| f.is_security());
    let mut reasons = Vec::new();
    match std {
        None => reasons.push("no finished code review".to_owned()),
        Some(r) if !reviewer_approved => reasons.push(format!(
            "the reviewer did not approve ({})",
            r.verdict.map_or("no verdict".to_owned(), |v| format!("{v:?}"))
        )),
        Some(_) => {}
    }
    for f in blocking.iter().take(5) {
        reasons.push(format!("unresolved critical {} finding: {}", f.category, one_line(&f.title, 120)));
    }
    let blocking_findings = u32::try_from(blocking.len()).unwrap_or(u32::MAX);
    ReviewGate {
        task_id: Some(task),
        reviewer_approved,
        no_critical_security,
        blocking_findings,
        latest_review_id: std.map(|r| r.id),
        latest_adversarial_id: adv.map(|r| r.id),
        passed: reviewer_approved && blocking_findings == 0,
        reasons,
    }
}

// ---------------------------------------------------------------------------
// service
// ---------------------------------------------------------------------------

impl Inner {
    /// Review persistence.
    pub(crate) fn reviews(&self) -> ReviewStore {
        ReviewStore::new(self.deps.db.clone())
    }

    /// Saves + publishes a review.
    pub(crate) async fn save_review(&self, review: &Review) {
        if let Err(e) = self.reviews().save_review(review).await {
            tracing::error!(review_id = %review.id, error = %e, "failed to persist a review");
            return;
        }
        self.deps
            .publish(
                NewEvent::new(EventKind::ReviewUpdated(ReviewPayload { review: review.clone() }))
                    .project(review.project_id)
                    .task(review.task_id),
            )
            .await;
    }

    /// Publishes `ReviewFinished` for a finished review.
    pub(crate) async fn review_finished(&self, review: &Review) {
        let Some(verdict) = review.verdict else { return };
        self.deps
            .publish(
                NewEvent::new(EventKind::ReviewFinished(ReviewFinishedPayload {
                    reviewer_agent_id: None,
                    verdict: review_state(verdict),
                    findings: u32::try_from(review.findings.len()).unwrap_or(u32::MAX),
                    summary: one_line(&review.summary, 500),
                }))
                .project(review.project_id)
                .task(review.task_id),
            )
            .await;
    }

    /// The emergency-aware cancel token of a project.
    pub(crate) fn quality_token(&self, project: ProjectId) -> OrchResult<CancellationToken> {
        if self.deps.security.emergency.flags().all_stopped {
            return Err(OrchestratorError::Forbidden(
                "Stop All AI is active — reset the emergency controls first".into(),
            ));
        }
        self.deps
            .security
            .emergency
            .register_project(project)
            .map(|t| t.child_token())
            .map_err(|e| OrchestratorError::Forbidden(e.to_string()))
    }

    /// A fresh review document.
    pub(crate) fn new_review(&self, task: &Task, kind: ReviewKind, model: Option<ModelRef>) -> Review {
        let at = now();
        Review {
            id: ReviewId::new(),
            project_id: task.project_id,
            task_id: task.id,
            kind,
            status: ReviewStatus::Running,
            verdict: None,
            summary: String::new(),
            findings: Vec::new(),
            ac_check: Vec::new(),
            model,
            author_model: task.model.clone(),
            same_model_fallback: false,
            verifier_model: None,
            diff: ReviewDiffStats::default(),
            transitioned_to: None,
            linked_review_id: None,
            usage: UsageSummary::default(),
            error: None,
            created_at: at,
            updated_at: at,
            finished_at: None,
        }
    }

    /// Runs a standard review to its end (never fails: errors land in the document).
    pub(crate) async fn run_standard(
        &self,
        mut review: Review,
        project: &Project,
        task: &Task,
        chain: Vec<ModelRef>,
        change: &Change,
        cancel: &CancellationToken,
    ) -> Review {
        review.diff = change.stats.clone();
        self.save_review(&review).await;
        let language = self.deps.config.ui_language.get();
        let brief = task_brief(self, project, task, change).await;
        let result = structured_call(
            self,
            project.id,
            Some(task.id),
            chain,
            review_system(&language),
            format!("{brief}\nReview this change now."),
            "orkestr_review",
            review_schema(),
            |doc| parse_review(doc, task),
            cancel,
        )
        .await;
        match result {
            Ok((parsed, s)) => {
                review.verdict = Some(parsed.verdict);
                review.summary = parsed.summary;
                review.findings = parsed.findings;
                review.ac_check = parsed.ac_check;
                review.model = Some(s.model);
                review.usage.add(&s.usage);
                review.status = ReviewStatus::Finished;
            }
            Err(e) => {
                review.status = ReviewStatus::Failed;
                review.error = Some(one_line(&e.to_string(), 500));
            }
        }
        review.updated_at = now();
        review.finished_at = Some(review.updated_at);
        self.save_review(&review).await;
        if review.status == ReviewStatus::Finished {
            self.review_finished(&review).await;
        }
        tracing::info!(review_id = %review.id, task_id = %task.id, status = ?review.status, verdict = ?review.verdict, "code review finished");
        review
    }

    /// Applies a finished review to the task: acceptance criteria met flags,
    /// review state and — for a task in `Review` with `transition` — the
    /// status change. Returns the new status when the task moved.
    pub(crate) async fn apply_review(
        &self,
        std: &mut Review,
        adversarial: Option<&Review>,
        transition: bool,
    ) -> Option<TaskStatus> {
        if std.status != ReviewStatus::Finished {
            return None;
        }
        let verdict = std.verdict?;
        let task = self.deps.tasks.get(std.task_id).await.ok()?;
        // Criteria the reviewer verified as met are ticked (never unticked).
        let mut criteria = task.acceptance_criteria.clone();
        let mut changed = false;
        for (c, check) in criteria.iter_mut().zip(std.ac_check.iter()) {
            if check.met && !c.met && check.criterion == c.text {
                c.met = true;
                changed = true;
            }
        }
        if changed {
            let patch = TaskPatch { acceptance_criteria: Some(criteria), ..TaskPatch::default() };
            if let Err(e) = self.deps.tasks.update(task.id, None, patch).await {
                tracing::warn!(task_id = %task.id, error = %e, "review: could not tick acceptance criteria");
            }
        }
        let adv_blocking: Vec<&ReviewFinding> =
            adversarial.filter(|a| a.status == ReviewStatus::Finished).map(|a| a.blocking()).unwrap_or_default();
        let effective = if verdict == ReviewVerdict::Approve && !adv_blocking.is_empty() {
            ReviewVerdict::RequestChanges
        } else {
            verdict
        };
        if let Err(e) = self.deps.tasks.set_review_state(task.id, review_state(effective)).await {
            tracing::warn!(task_id = %task.id, error = %e, "review: could not set the review state");
        }
        if !transition || task.status != TaskStatus::Review {
            return None;
        }
        let to = match effective {
            ReviewVerdict::Approve => TaskStatus::Testing,
            ReviewVerdict::RequestChanges => TaskStatus::Running,
            ReviewVerdict::Reject => TaskStatus::Failed,
        };
        let mut top: Vec<String> = std
            .findings
            .iter()
            .chain(adv_blocking.iter().copied())
            .filter(|f| f.status.is_unresolved() && f.severity <= FindingSeverity::Medium)
            .take(3)
            .map(|f| one_line(&f.title, 80))
            .collect();
        top.dedup();
        let reason = match effective {
            ReviewVerdict::Approve => "code review approved".to_owned(),
            ReviewVerdict::RequestChanges if !adv_blocking.is_empty() && verdict == ReviewVerdict::Approve => {
                format!("adversarial review found unresolved critical issues: {}", top.join("; "))
            }
            ReviewVerdict::RequestChanges => format!("code review requested changes: {}", top.join("; ")),
            ReviewVerdict::Reject => format!("code review rejected the change: {}", one_line(&std.summary, 200)),
        };
        match self.deps.tasks.transition(task.id, to, TaskActor::Manager, Some(cap(&reason, MAX_REASON)), None).await {
            Ok(_) => {
                std.transitioned_to = Some(to);
                std.updated_at = now();
                self.save_review(std).await;
                let ru = is_ru(&self.deps.config.ui_language.get());
                let text = match (effective, ru) {
                    (ReviewVerdict::Approve, false) => {
                        format!("Code review approved \"{}\" — it moves to testing.", task.title)
                    }
                    (ReviewVerdict::Approve, true) => {
                        format!("Код-ревью одобрило задачу «{}» — она переходит к тестированию.", task.title)
                    }
                    (ReviewVerdict::RequestChanges, false) => {
                        format!("Code review requested changes to \"{}\": {}", task.title, top.join("; "))
                    }
                    (ReviewVerdict::RequestChanges, true) => {
                        format!("Код-ревью запросило доработку задачи «{}»: {}", task.title, top.join("; "))
                    }
                    (ReviewVerdict::Reject, false) => format!("Code review rejected \"{}\".", task.title),
                    (ReviewVerdict::Reject, true) => format!("Код-ревью отклонило задачу «{}».", task.title),
                };
                self.system_event(task.project_id, &text, MessageExtras::default()).await;
                Some(to)
            }
            Err(e) => {
                tracing::warn!(task_id = %task.id, error = %e, "review: task transition failed");
                None
            }
        }
    }
}

/// Task review state for a verdict.
#[must_use]
pub fn review_state(v: ReviewVerdict) -> ReviewState {
    match v {
        ReviewVerdict::Approve => ReviewState::Approved,
        ReviewVerdict::RequestChanges => ReviewState::ChangesRequested,
        ReviewVerdict::Reject => ReviewState::Rejected,
    }
}

impl Manager {
    /// `reviews.request`: automatic code review of a task, optionally together
    /// with an adversarial review. Returns at once (status `Running`) unless
    /// `wait` is set; progress arrives as `ReviewUpdated` events.
    pub async fn request_review(&self, p: RequestReviewParams) -> OrchResult<ReviewResult> {
        let inner = &self.inner;
        let task = inner.deps.tasks.get(p.task_id).await?;
        let project = inner.project(task.project_id).await?;
        let cancel = inner.quality_token(project.id)?;
        let chain = role_chain(inner, &project, "Review", p.model.as_ref(), &[]).await?;
        let guard = InFlight::acquire(task.id, false)?;
        let adversarial = if p.adversarial.unwrap_or(false) {
            Some(crate::adversarial::prepare(inner, &project, &task, None).await?)
        } else {
            None
        };
        let mut review = inner.new_review(&task, ReviewKind::Standard, chain.first().cloned());
        let mut adv_review = adversarial.as_ref().map(|a| {
            let mut r = a.review.clone();
            r.linked_review_id = Some(review.id);
            r
        });
        review.linked_review_id = adv_review.as_ref().map(|r| r.id);
        inner.save_review(&review).await;
        if let Some(r) = &adv_review {
            inner.save_review(r).await;
        }
        let transition = p.transition.unwrap_or(true);
        let mgr = self.clone();
        let (std0, adv0) = (review.clone(), adv_review.clone());
        let handle = tokio::spawn(async move {
            let _guard = guard;
            let inner = &mgr.inner;
            let change = gather_change(inner, &project, &task).await;
            let adv_fut = async {
                match (adversarial, adv0) {
                    (Some(mut plan), Some(doc)) => {
                        plan.review = doc;
                        Some(crate::adversarial::run(inner, plan, &project, &task, &change, &cancel).await)
                    }
                    _ => None,
                }
            };
            let (mut std, adv) =
                tokio::join!(inner.run_standard(std0, &project, &task, chain, &change, &cancel), adv_fut);
            inner.apply_review(&mut std, adv.as_ref(), transition).await;
        });
        if p.wait.unwrap_or(false) {
            handle.await.map_err(|e| OrchestratorError::Internal(format!("review task failed: {e}")))?;
            review = inner.reviews().review(review.id).await?;
            if let Some(r) = &adv_review {
                adv_review = Some(inner.reviews().review(r.id).await?);
            }
        }
        Ok(ReviewResult { review, adversarial: adv_review })
    }

    /// `reviews.get`.
    pub async fn get_review(&self, id: ReviewId) -> OrchResult<ReviewResult> {
        let store = self.inner.reviews();
        let review = store.review(id).await?;
        let adversarial = match (review.kind, review.linked_review_id) {
            (ReviewKind::Standard, Some(l)) => store.review(l).await.ok(),
            _ => None,
        };
        Ok(ReviewResult { review, adversarial })
    }

    /// `reviews.list`.
    pub async fn list_reviews(&self, p: ListReviewsParams) -> OrchResult<ListReviewsResult> {
        if p.project_id.is_none() && p.task_id.is_none() {
            return Err(OrchestratorError::Invalid("give a projectId or a taskId".into()));
        }
        let limit = p.limit.unwrap_or(50).clamp(1, 500);
        let reviews = self.inner.reviews().reviews(p.project_id, p.task_id, limit).await?;
        let gate = p.task_id.map(|t| gate_of(t, &reviews));
        Ok(ListReviewsResult { reviews, gate })
    }

    /// Quality-gate view of a task's latest reviews.
    pub async fn review_gate(&self, task: TaskId) -> OrchResult<ReviewGate> {
        let reviews = self.inner.reviews().reviews(None, Some(task), 50).await?;
        Ok(gate_of(task, &reviews))
    }

    /// Unresolved findings of the task's latest reviews as markdown — for the
    /// next agent attempt after "changes requested". `None` when nothing is open.
    pub async fn review_feedback(&self, task: TaskId) -> OrchResult<Option<String>> {
        let reviews = self.inner.reviews().reviews(None, Some(task), 20).await?;
        let mut out = String::new();
        for kind in [ReviewKind::Standard, ReviewKind::Adversarial] {
            let Some(r) = reviews.iter().find(|r| r.kind == kind && r.status == ReviewStatus::Finished) else {
                continue;
            };
            let open: Vec<&ReviewFinding> = r
                .findings
                .iter()
                .filter(|f| f.status.is_unresolved() && f.severity <= FindingSeverity::Medium)
                .collect();
            let unmet: Vec<&AcCheck> = r.ac_check.iter().filter(|c| !c.met).collect();
            if open.is_empty() && unmet.is_empty() {
                continue;
            }
            out.push_str(&format!("## {kind:?} review: {}\n", one_line(&r.summary, 300)));
            for f in open {
                let at = match (&f.file, f.line) {
                    (Some(file), Some(l)) => format!(" ({file}:{l})"),
                    (Some(file), None) => format!(" ({file})"),
                    _ => String::new(),
                };
                out.push_str(&format!(
                    "- [{:?}/{}] {}{at}: {} — fix: {}\n",
                    f.severity,
                    f.category,
                    f.title,
                    one_line(&f.detail, 300),
                    one_line(&f.suggestion, 200)
                ));
            }
            for c in unmet {
                out.push_str(&format!("- Acceptance criterion not met: {}\n", one_line(&c.criterion, 200)));
            }
        }
        Ok((!out.is_empty()).then_some(out))
    }

    /// `reviews.resolveFinding`: marks a finding fixed / dismissed / open again.
    pub async fn resolve_finding(&self, p: ResolveFindingParams) -> OrchResult<ReviewResult> {
        if !matches!(p.status, FindingStatus::Fixed | FindingStatus::Dismissed | FindingStatus::Open) {
            return Err(OrchestratorError::Invalid("a finding can only be marked Fixed, Dismissed or Open".into()));
        }
        let mut review = self.inner.reviews().review(p.review_id).await?;
        if !review.status.is_final() {
            return Err(OrchestratorError::Conflict("the review is still running".into()));
        }
        let note = p.note.as_deref().map(str::trim).filter(|n| !n.is_empty()).map(|n| cap(n, 2_000));
        let finding = review
            .findings
            .iter_mut()
            .find(|f| f.id == p.finding_id)
            .ok_or_else(|| OrchestratorError::NotFound(format!("finding {}", p.finding_id)))?;
        if p.status == FindingStatus::Dismissed && finding.severity == FindingSeverity::Critical && note.is_none() {
            return Err(OrchestratorError::Invalid("dismissing a critical finding needs a note".into()));
        }
        finding.status = p.status;
        finding.resolution_note = note;
        finding.resolved_at = (p.status != FindingStatus::Open).then(now);
        review.updated_at = now();
        self.inner.save_review(&review).await;
        self.get_review(review.id).await
    }

    /// Marks reviews and consensus runs of a previous session that never
    /// finished as `Failed` (crash recovery).
    pub async fn recover_reviews(&self) -> OrchResult<()> {
        let store = self.inner.reviews();
        for mut r in store.unfinished_reviews().await? {
            r.status = ReviewStatus::Failed;
            r.error = Some("Orkestr stopped while the review was running — request it again".into());
            r.updated_at = now();
            r.finished_at = Some(r.updated_at);
            store.save_review(&r).await?;
        }
        for mut c in store.unfinished_consensus().await? {
            c.status = orkestr_protocol::methods::consensus::ConsensusStatus::Failed;
            c.error = Some("Orkestr stopped while the consensus run was active — run it again".into());
            c.updated_at = now();
            c.finished_at = Some(c.updated_at);
            store.save_consensus(&c).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn extracts_objects_from_noisy_text() {
        assert_eq!(extract_object(r#"{"a":1}"#).unwrap()["a"], 1);
        assert_eq!(extract_object("Sure:\n```json\n{\"a\":2}\n```").unwrap()["a"], 2);
        assert_eq!(extract_object(r#"x {"a":"}{","b":{"c":3}} y"#).unwrap()["b"]["c"], 3);
        assert!(extract_object("no json").is_none());
    }

    #[test]
    fn lenient_verdicts_and_severities() {
        assert_eq!(parse_verdict(Some(&json!("Changes-Requested"))), Some(ReviewVerdict::RequestChanges));
        assert_eq!(parse_verdict(Some(&json!("LGTM"))), Some(ReviewVerdict::Approve));
        assert_eq!(parse_verdict(Some(&json!("maybe"))), None);
        assert_eq!(parse_severity(Some(&json!("Blocker"))), FindingSeverity::Critical);
        assert_eq!(parse_severity(None), FindingSeverity::Info);
    }

    #[test]
    fn approval_with_a_critical_finding_is_withdrawn() {
        let task = Task::new(ProjectId::new(), "t", now());
        let doc = json!({"verdict": "approve", "summary": "ok", "findings": [
            {"severity": "critical", "category": "security", "file": "a.rs", "line": 3, "title": "leak",
             "detail": "", "suggestion": "", "evidence": "x"}
        ], "acceptanceCriteria": []});
        let p = parse_review(&doc, &task).unwrap();
        assert_eq!(p.verdict, ReviewVerdict::RequestChanges);
        assert_eq!(p.findings[0].id, "f1");
        assert!(parse_review(&json!({"summary": "x"}), &task).is_err());
    }

    #[test]
    fn gate_requires_approval_and_no_blockers() {
        let task = Task::new(ProjectId::new(), "t", now());
        assert!(!gate_of(task.id, &[]).passed);
        let inner_review = |kind, verdict, sev: FindingSeverity, cat: &str| {
            let at = now();
            Review {
                id: ReviewId::new(),
                project_id: task.project_id,
                task_id: task.id,
                kind,
                status: ReviewStatus::Finished,
                verdict: Some(verdict),
                summary: String::new(),
                findings: vec![ReviewFinding {
                    id: "f1".into(),
                    severity: sev,
                    category: cat.into(),
                    file: None,
                    line: None,
                    title: "x".into(),
                    detail: String::new(),
                    suggestion: String::new(),
                    evidence: String::new(),
                    status: FindingStatus::Open,
                    response: None,
                    resolution_note: None,
                    resolved_at: None,
                }],
                ac_check: Vec::new(),
                model: None,
                author_model: None,
                same_model_fallback: false,
                verifier_model: None,
                diff: ReviewDiffStats::default(),
                transitioned_to: None,
                linked_review_id: None,
                usage: UsageSummary::default(),
                error: None,
                created_at: at,
                updated_at: at,
                finished_at: Some(at),
            }
        };
        let ok = inner_review(ReviewKind::Standard, ReviewVerdict::Approve, FindingSeverity::Low, "style");
        let g = gate_of(task.id, std::slice::from_ref(&ok));
        assert!(g.passed && g.reviewer_approved && g.no_critical_security);
        let adv =
            inner_review(ReviewKind::Adversarial, ReviewVerdict::RequestChanges, FindingSeverity::Critical, "security");
        let g = gate_of(task.id, &[adv, ok]);
        assert!(!g.passed && g.reviewer_approved && !g.no_critical_security);
        assert_eq!(g.blocking_findings, 1);
    }
}
