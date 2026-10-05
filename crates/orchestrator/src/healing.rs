//! Self-Healing (§17, §68.15–16): failure analysis, no-progress detection,
//! limits and the fix briefing of the quality pipeline ([`crate::pipeline`]).
//!
//! Loop: Implement → Build → Test → **Error → Analyze** (structured summary
//! from the quality run: failing tests, compiler errors with file:line,
//! relevant code excerpts through the context optimizer) → **Fix** (same
//! model, escalated model from the second attempt, via the agent handoff +
//! a briefing) → Test → Review. Everything here is pure or bounded I/O
//! (reading a few files of the worktree), so it is unit-tested directly.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use orkestr_protocol::methods::pipeline::{
    CodeExcerpt, FailureAnalysis, HealingAttempt, HealingLimits, HealingOutcome, PipelineStageKind, MAX_EXCERPTS,
    MAX_FAILURE_ITEMS,
};
use orkestr_protocol::methods::quality::{CheckStatus, Diagnostic, DiagnosticSeverity, QualityRun, TestFailure};
use orkestr_protocol::methods::reviews::{FindingSeverity, Review, ReviewFinding};
use orkestr_providers::adapters::mock::HEALING_BRIEF_HEADING;

use crate::util::{cap, one_line};

/// Lines shown around a failure line.
const WINDOW: u32 = 4;
/// Max characters of one excerpt.
const MAX_EXCERPT_CHARS: usize = 2_000;
/// Max bytes of a file read for an excerpt.
const MAX_FILE_BYTES: u64 = 512 * 1024;

// ---------------------------------------------------------------------------
// analysis
// ---------------------------------------------------------------------------

/// Analysis of a failed quality run (Build or Test stage).
#[must_use]
pub fn analyze_checks(
    stage: PipelineStageKind,
    run: &QualityRun,
    root: Option<&Path>,
    task_text: &str,
) -> FailureAnalysis {
    let failing: Vec<_> = run
        .results
        .iter()
        .filter(|r| matches!(r.status, CheckStatus::Failed | CheckStatus::TimedOut | CheckStatus::Error))
        .collect();
    let mut tests: Vec<TestFailure> = Vec::new();
    let mut errors: Vec<Diagnostic> = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    for r in &failing {
        tests.extend(r.failures.iter().cloned());
        errors.extend(r.diagnostics.iter().filter(|d| d.severity == DiagnosticSeverity::Error).cloned());
        let what = if r.summary.trim().is_empty() {
            format!("{:?} (exit {})", r.status, r.exit_code.map_or_else(|| "?".to_owned(), |c| c.to_string()))
        } else {
            one_line(&r.summary, 200)
        };
        lines.push(format!("`{}` — {what}", r.command));
    }
    tests.truncate(MAX_FAILURE_ITEMS);
    errors.truncate(MAX_FAILURE_ITEMS);
    let mut summary = match stage {
        PipelineStageKind::Build => format!("The build failed: {}.", lines.join("; ")),
        _ => format!("The tests failed: {}.", lines.join("; ")),
    };
    if !tests.is_empty() {
        summary.push_str(&format!(" {} failing test(s).", tests.len()));
    }
    if !errors.is_empty() {
        summary.push_str(&format!(" {} compiler error(s).", errors.len()));
    }
    if failing.is_empty() {
        summary = format!("The checks did not pass ({:?}).", run.status);
    }
    // Raw output tail when the parser found nothing structured.
    let raw: Vec<String> = if tests.is_empty() && errors.is_empty() {
        failing.iter().map(|r| cap(r.raw_tail.trim(), 1_500)).filter(|t| !t.is_empty()).collect()
    } else {
        Vec::new()
    };
    let mut places: Vec<(String, u32)> = Vec::new();
    for t in &tests {
        if let (Some(f), Some(l)) = (&t.file, t.line) {
            places.push((f.clone(), l));
        }
    }
    for d in &errors {
        if let (Some(f), Some(l)) = (&d.file, d.line) {
            places.push((f.clone(), l));
        }
    }
    let mut query = format!("{task_text}\n{summary}");
    for t in &tests {
        query.push_str(&format!("\n{} {}", t.name, t.message));
    }
    for d in &errors {
        query.push_str(&format!("\n{}", d.message));
    }
    let excerpts = root.map(|r| code_excerpts(r, &places, &query)).unwrap_or_default();
    let signature = signature(stage, &tests, &errors, &[], &raw);
    FailureAnalysis {
        stage,
        summary: cap(&summary, 1_000),
        signature,
        failing_tests: tests,
        compiler_errors: errors,
        findings: raw.into_iter().map(|r| format!("Output:\n{r}")).collect(),
        excerpts,
        source_id: Some(run.id.clone()),
    }
}

/// Unresolved findings of a review that block it (review: medium and worse;
/// adversarial: the blocking ones).
#[must_use]
pub fn open_findings(review: &Review, adversarial: bool) -> Vec<&ReviewFinding> {
    if adversarial {
        review.blocking()
    } else {
        review.findings.iter().filter(|f| f.status.is_unresolved() && f.severity <= FindingSeverity::Medium).collect()
    }
}

/// Analysis of a review that requested changes (Review stage) or found
/// blocking issues (Security stage).
#[must_use]
pub fn analyze_review(
    stage: PipelineStageKind,
    review: &Review,
    root: Option<&Path>,
    task_text: &str,
) -> FailureAnalysis {
    let adversarial = stage == PipelineStageKind::Security;
    let open = open_findings(review, adversarial);
    let findings: Vec<String> = open
        .iter()
        .take(MAX_FAILURE_ITEMS)
        .map(|f| {
            let at = match (&f.file, f.line) {
                (Some(file), Some(l)) => format!(" ({file}:{l})"),
                (Some(file), None) => format!(" ({file})"),
                _ => String::new(),
            };
            format!(
                "[{:?}/{}] {}{at}: {} — fix: {}",
                f.severity,
                f.category,
                one_line(&f.title, 160),
                one_line(&f.detail, 300),
                one_line(&f.suggestion, 200)
            )
        })
        .collect();
    let unmet: Vec<String> = review
        .ac_check
        .iter()
        .filter(|c| !c.met)
        .map(|c| format!("Acceptance criterion not met: {}", one_line(&c.criterion, 200)))
        .collect();
    let places: Vec<(String, u32)> = open.iter().filter_map(|f| Some((f.file.clone()?, f.line?))).collect();
    let mut all = findings.clone();
    all.extend(unmet);
    let summary = match stage {
        PipelineStageKind::Security => format!(
            "The adversarial review found {} unresolved critical issue(s): {}",
            open.len(),
            one_line(&review.summary, 300)
        ),
        _ => format!("The code review requested changes: {}", one_line(&review.summary, 300)),
    };
    let query = format!("{task_text}\n{summary}\n{}", all.join("\n"));
    let excerpts = root.map(|r| code_excerpts(r, &places, &query)).unwrap_or_default();
    let keys: Vec<String> = open.iter().map(|f| format!("{}:{}", f.category, f.title)).collect();
    FailureAnalysis {
        stage,
        summary: cap(&summary, 1_000),
        signature: signature(stage, &[], &[], &keys, &[]),
        failing_tests: Vec::new(),
        compiler_errors: Vec::new(),
        findings: all,
        excerpts,
        source_id: Some(review.id.to_string()),
    }
}

/// Lowercase, digits collapsed to `#`, whitespace squeezed: line numbers,
/// durations and addresses do not make two failures look different.
#[must_use]
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_digit = false;
    let mut last_space = false;
    for c in s.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_digit() {
            if !last_digit {
                out.push('#');
            }
            last_digit = true;
            last_space = false;
        } else if c.is_whitespace() {
            if !last_space {
                out.push(' ');
            }
            last_space = true;
            last_digit = false;
        } else {
            out.push(c);
            last_digit = false;
            last_space = false;
        }
    }
    out.trim().to_owned()
}

/// Fingerprint of a failure: stage + the sorted set of normalised failing
/// test names / messages, compiler errors (file, code, message), finding keys
/// and (when nothing is structured) the raw output. FNV-1a, hex.
#[must_use]
pub fn signature(
    stage: PipelineStageKind,
    tests: &[TestFailure],
    errors: &[Diagnostic],
    findings: &[String],
    raw: &[String],
) -> String {
    let mut items: BTreeSet<String> = BTreeSet::new();
    for t in tests {
        items.insert(format!("test:{}:{}", normalize(&t.name), normalize(&one_line(&t.message, 200))));
    }
    for d in errors {
        items.insert(format!(
            "diag:{}:{}:{}",
            d.file.as_deref().map(normalize).unwrap_or_default(),
            d.code.as_deref().unwrap_or_default(),
            normalize(&one_line(&d.message, 200))
        ));
    }
    for f in findings {
        items.insert(format!("finding:{}", normalize(f)));
    }
    if items.is_empty() {
        for r in raw {
            // The last lines of the output carry the error; timings vary.
            let tail: Vec<&str> = r.lines().rev().take(6).collect();
            items.insert(format!("raw:{}", normalize(&tail.join("\n"))));
        }
    }
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in format!("{stage:?}|{}", items.into_iter().collect::<Vec<_>>().join("|")).bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// Reads a project-relative file under `root` (never outside it).
fn read_rel(root: &Path, rel: &str) -> Option<String> {
    let rel = rel.replace('\\', "/");
    let rel = rel.trim_start_matches("./");
    if rel.is_empty() || rel.starts_with('/') || rel.contains("..") || rel.contains(':') {
        return None;
    }
    let path = root.join(rel);
    let meta = std::fs::metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

/// Code around the failure lines (exact windows), then files the context
/// optimizer ranks for the failure (focus on the failing files), bounded by
/// [`MAX_EXCERPTS`].
#[must_use]
pub fn code_excerpts(root: &Path, places: &[(String, u32)], query: &str) -> Vec<CodeExcerpt> {
    let mut out: Vec<CodeExcerpt> = Vec::new();
    let mut seen: BTreeSet<(String, u32)> = BTreeSet::new();
    for (file, line) in places {
        if out.len() >= MAX_EXCERPTS {
            break;
        }
        let rel = file.replace('\\', "/").trim_start_matches("./").to_owned();
        if !seen.insert((rel.clone(), *line)) {
            continue;
        }
        let Some(text) = read_rel(root, &rel) else { continue };
        let all: Vec<&str> = text.lines().collect();
        if all.is_empty() {
            continue;
        }
        let total = u32::try_from(all.len()).unwrap_or(u32::MAX);
        let at = (*line).clamp(1, total);
        let start = at.saturating_sub(WINDOW).max(1);
        let end = (at + WINDOW).min(total);
        let body: Vec<String> = (start..=end)
            .map(|n| {
                let l = all.get((n - 1) as usize).copied().unwrap_or_default();
                format!("{}{n:>4} | {l}", if n == at { ">" } else { " " })
            })
            .collect();
        out.push(CodeExcerpt {
            file: rel,
            start_line: start,
            end_line: end,
            text: cap(&body.join("\n"), MAX_EXCERPT_CHARS),
            reason: "failure line".into(),
        });
    }
    if out.len() < MAX_EXCERPTS {
        let focus: Vec<String> = places.iter().map(|(f, _)| f.replace('\\', "/")).collect();
        if let Ok(index) = orkestr_context::FileIndex::build(root, orkestr_context::IndexOptions::default()) {
            let reader = orkestr_context::FsReader::new(root);
            let req = orkestr_context::OptimizeRequest {
                task: cap(query, 4_000),
                token_budget: 1_800,
                max_files: 3,
                focus_paths: focus,
                ..orkestr_context::OptimizeRequest::default()
            };
            let plan = orkestr_context::optimize(&index, &reader, &req, orkestr_context::Signals::default());
            for f in plan.bundle.files {
                if out.len() >= MAX_EXCERPTS {
                    break;
                }
                let Some(e) = f.excerpts.first() else { continue };
                if out.iter().any(|o| o.file == f.path && o.start_line <= e.end_line && e.start_line <= o.end_line) {
                    continue;
                }
                out.push(CodeExcerpt {
                    file: f.path.clone(),
                    start_line: e.start_line,
                    end_line: e.end_line,
                    text: cap(&e.text, MAX_EXCERPT_CHARS),
                    reason: "context optimizer".into(),
                });
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// limits
// ---------------------------------------------------------------------------

/// Why the loop stops and asks the user.
#[derive(Debug, Clone, PartialEq)]
pub enum Escalation {
    /// Self-healing is disabled (0 attempts).
    Disabled,
    /// Every allowed attempt was used.
    Attempts {
        /// Attempts used.
        used: u32,
        /// Allowed.
        allowed: u32,
    },
    /// The pipeline's wall time is over.
    WallTime {
        /// Minutes.
        mins: u32,
    },
    /// The task's budget is spent.
    Budget {
        /// Spent (USD).
        spent: f64,
        /// Limit (USD).
        limit: f64,
    },
    /// A fix did not change the failure.
    NoProgress {
        /// Repeated signature.
        signature: String,
    },
}

impl Escalation {
    /// English reason (stored on the pipeline; the UI localises by kind).
    #[must_use]
    pub fn reason(&self) -> String {
        match self {
            Self::Disabled => "self-healing is disabled for this project".into(),
            Self::Attempts { used, allowed } => format!("{used} of {allowed} fix attempt(s) used"),
            Self::WallTime { mins } => format!("the pipeline's time limit of {mins} min is reached"),
            Self::Budget { spent, limit } => format!("the task's budget is spent (${spent:.4} of ${limit:.2})"),
            Self::NoProgress { .. } => "no progress: the last fix produced the same failure".into(),
        }
    }
}

/// Facts the decision needs.
#[derive(Debug, Clone)]
pub struct HealState<'a> {
    /// Limits.
    pub limits: &'a HealingLimits,
    /// Extra attempts granted by the user.
    pub granted: u32,
    /// Attempts so far.
    pub attempts: &'a [HealingAttempt],
    /// Signature of the failure to fix.
    pub signature: &'a str,
    /// Time since the pipeline started.
    pub elapsed: Duration,
    /// Task spend since the pipeline started (USD).
    pub spent: f64,
    /// Budget limit (USD): the smaller of the pipeline limit and the task's / run's budget.
    pub budget: Option<f64>,
    /// A configured budget (cost engine) is already exceeded.
    pub budget_exceeded: bool,
    /// The user just granted more attempts: skip the no-progress check once.
    pub forced: bool,
}

/// What to do with a failure.
#[derive(Debug, Clone, PartialEq)]
pub enum HealDecision {
    /// Start fix attempt `attempt` (1-based); `escalate_model` = use another model.
    Attempt {
        /// Attempt number.
        attempt: u32,
        /// Pick a different model than the previous attempt.
        escalate_model: bool,
    },
    /// Stop and ask the user.
    Escalate(Escalation),
}

/// The Self-Healing limits, in order: disabled, no progress, attempts, wall
/// time, budget.
#[must_use]
pub fn decide(s: &HealState<'_>) -> HealDecision {
    let allowed = s.limits.max_attempts.saturating_add(s.granted);
    if allowed == 0 {
        return HealDecision::Escalate(Escalation::Disabled);
    }
    let used = u32::try_from(s.attempts.len()).unwrap_or(u32::MAX);
    if s.limits.stop_on_repeat && !s.forced {
        if let Some(last) = s.attempts.last() {
            let applied = matches!(
                last.outcome,
                HealingOutcome::Submitted | HealingOutcome::StillFailing | HealingOutcome::Fixed
            );
            if applied && last.analysis.signature == s.signature {
                return HealDecision::Escalate(Escalation::NoProgress { signature: s.signature.to_owned() });
            }
        }
    }
    if used >= allowed {
        return HealDecision::Escalate(Escalation::Attempts { used, allowed });
    }
    let mins = s.limits.max_wall_mins;
    if mins > 0 && s.elapsed >= Duration::from_secs(u64::from(mins) * 60) {
        return HealDecision::Escalate(Escalation::WallTime { mins });
    }
    if let Some(limit) = s.budget {
        if s.spent >= limit {
            return HealDecision::Escalate(Escalation::Budget { spent: s.spent, limit });
        }
    }
    if s.budget_exceeded {
        return HealDecision::Escalate(Escalation::Budget { spent: s.spent, limit: s.budget.unwrap_or(0.0) });
    }
    let attempt = used + 1;
    HealDecision::Attempt { attempt, escalate_model: s.limits.escalate_model && attempt >= 2 }
}

// ---------------------------------------------------------------------------
// briefing
// ---------------------------------------------------------------------------

/// The fix agent's briefing (placed after the task header of its first
/// message; the previous handoff is injected by the runtime as usual).
#[must_use]
pub fn briefing(a: &FailureAnalysis, attempt: u32, allowed: u32) -> String {
    let mut s = format!("{HEALING_BRIEF_HEADING} (attempt {attempt} of {allowed})\n\n");
    s.push_str(&format!(
        "The quality pipeline stopped at the **{:?}** stage. Fix the cause in this worktree (your branch already \
         contains the previous attempt), keep the change minimal, re-run the failing check if you can, commit, \
         then call submit_result. Do not disable or delete tests.\n\n",
        a.stage
    ));
    s.push_str(&format!("**Failure:** {}\n", a.summary));
    if !a.failing_tests.is_empty() {
        s.push_str("\n## Failing tests\n");
        for t in &a.failing_tests {
            let at = match (&t.file, t.line) {
                (Some(f), Some(l)) => format!(" ({f}:{l})"),
                (Some(f), None) => format!(" ({f})"),
                _ => String::new(),
            };
            s.push_str(&format!("- `{}`{at}: {}\n", one_line(&t.name, 160), one_line(&t.message, 400)));
        }
    }
    if !a.compiler_errors.is_empty() {
        s.push_str("\n## Compiler errors\n");
        for d in &a.compiler_errors {
            let at = match (&d.file, d.line) {
                (Some(f), Some(l)) => format!("{f}:{l}"),
                (Some(f), None) => f.clone(),
                _ => "?".into(),
            };
            let code = d.code.as_deref().map(|c| format!(" [{c}]")).unwrap_or_default();
            s.push_str(&format!("- {at}{code}: {}\n", one_line(&d.message, 400)));
        }
    }
    if !a.findings.is_empty() {
        s.push_str("\n## To address\n");
        for f in a.findings.iter().take(12) {
            s.push_str(&format!("- {}\n", cap(f, 1_600)));
        }
    }
    if !a.excerpts.is_empty() {
        s.push_str("\n## Relevant code\n");
        for e in &a.excerpts {
            s.push_str(&format!(
                "`{}` lines {}–{} ({}):\n```\n{}\n```\n",
                e.file, e.start_line, e.end_line, e.reason, e.text
            ));
        }
    }
    cap(&s, 24_000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_protocol::methods::quality::{CheckKind, CheckResult};

    fn failure(name: &str, msg: &str, file: &str, line: u32) -> TestFailure {
        TestFailure { name: name.into(), message: msg.into(), file: Some(file.into()), line: Some(line) }
    }

    fn quality_run(results: Vec<CheckResult>) -> QualityRun {
        serde_json::from_value(serde_json::json!({
            "id": "q1",
            "projectId": orkestr_domain::ProjectId::new(),
            "target": { "type": "project" },
            "workspace": ".",
            "kinds": ["Test"],
            "status": "Failed",
            "results": results,
            "startedAt": orkestr_domain::now(),
            "durationMs": 3,
        }))
        .unwrap()
    }

    fn check(kind: CheckKind, failures: Vec<TestFailure>, diagnostics: Vec<Diagnostic>) -> CheckResult {
        let mut c: CheckResult = serde_json::from_value(serde_json::json!({
            "checkId": "c", "kind": kind, "toolchain": "Cargo", "label": "tests", "command": "npm test", "cwd": ".",
            "status": "Failed", "durationMs": 1, "passed": 0, "failed": 1, "skipped": 0,
            "failures": [], "diagnostics": [], "rawTail": "not ok 1", "summary": "1 failed",
        }))
        .unwrap();
        c.failures = failures;
        c.diagnostics = diagnostics;
        c
    }

    #[test]
    fn signature_ignores_numbers_and_order() {
        let a = [failure("greets", "expected 'a' at 12ms", "src/a.mjs", 3), failure("b", "x", "src/b.mjs", 1)];
        let b = [failure("b", "x", "src/b.mjs", 9), failure("greets", "expected 'a' at 40ms", "src/a.mjs", 7)];
        assert_eq!(
            signature(PipelineStageKind::Test, &a, &[], &[], &[]),
            signature(PipelineStageKind::Test, &b, &[], &[], &[])
        );
        let c = [failure("greets", "expected 'b'", "src/a.mjs", 3)];
        assert_ne!(
            signature(PipelineStageKind::Test, &a, &[], &[], &[]),
            signature(PipelineStageKind::Test, &c, &[], &[], &[])
        );
        assert_ne!(
            signature(PipelineStageKind::Test, &c, &[], &[], &[]),
            signature(PipelineStageKind::Build, &c, &[], &[], &[]),
            "stage is part of the signature"
        );
        assert_eq!(normalize("Line 12:  Expected\t3"), "line #: expected #");
    }

    #[test]
    fn analysis_collects_tests_errors_and_excerpts() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        let body: String = (1..=20).map(|i| format!("line {i}\n")).collect();
        std::fs::write(dir.path().join("src/a.mjs"), &body).unwrap();
        let diag = Diagnostic {
            severity: DiagnosticSeverity::Error,
            message: "type mismatch".into(),
            file: Some("src/a.mjs".into()),
            line: Some(10),
            column: None,
            code: Some("TS2322".into()),
        };
        let warn = Diagnostic { severity: DiagnosticSeverity::Warning, ..diag.clone() };
        let run = quality_run(vec![check(
            CheckKind::Test,
            vec![failure("greets by name", "expected 'Hello, X!'", "src/a.mjs", 3)],
            vec![diag, warn],
        )]);
        let a = analyze_checks(PipelineStageKind::Test, &run, Some(dir.path()), "greet");
        assert_eq!(a.failing_tests.len(), 1);
        assert_eq!(a.compiler_errors.len(), 1, "warnings are not errors");
        assert!(a.summary.contains("1 failing test"), "{}", a.summary);
        assert_eq!(a.source_id.as_deref(), Some("q1"));
        let first = &a.excerpts[0];
        assert_eq!((first.file.as_str(), first.start_line, first.end_line), ("src/a.mjs", 1, 7));
        assert!(first.text.contains(">   3 | line 3"), "{}", first.text);
        assert!(a.excerpts.iter().any(|e| e.start_line == 6 && e.end_line == 14));
        // Paths outside the worktree are never read.
        assert!(code_excerpts(dir.path(), &[("../secret".into(), 1)], "x").iter().all(|e| e.file != "../secret"));
        let brief = briefing(&a, 2, 3);
        assert!(brief.starts_with("# Self-healing fix (attempt 2 of 3)"), "{brief}");
        assert!(brief.contains("src/a.mjs:10 [TS2322]: type mismatch"), "{brief}");
        assert!(brief.contains("greets by name"));
    }

    fn attempt(sig: &str, outcome: HealingOutcome) -> HealingAttempt {
        HealingAttempt {
            attempt: 1,
            analysis: FailureAnalysis {
                stage: PipelineStageKind::Test,
                summary: String::new(),
                signature: sig.into(),
                failing_tests: vec![],
                compiler_errors: vec![],
                findings: vec![],
                excerpts: vec![],
                source_id: None,
            },
            model: None,
            escalated: false,
            agent_id: None,
            outcome,
            note: None,
            started_at: orkestr_domain::now(),
            finished_at: None,
        }
    }

    #[test]
    fn limits_are_enforced_in_order() {
        let limits =
            HealingLimits { max_attempts: 2, max_wall_mins: 10, max_budget_usd: Some(1.0), ..HealingLimits::default() };
        let base = |attempts: &'static [HealingAttempt]| HealState {
            limits: &limits,
            granted: 0,
            attempts,
            signature: "s2",
            elapsed: Duration::from_secs(5),
            spent: 0.1,
            budget: Some(1.0),
            budget_exceeded: false,
            forced: false,
        };
        assert_eq!(decide(&base(&[])), HealDecision::Attempt { attempt: 1, escalate_model: false });
        let one: &'static [HealingAttempt] = Box::leak(Box::new([attempt("s1", HealingOutcome::StillFailing)]));
        assert_eq!(decide(&base(one)), HealDecision::Attempt { attempt: 2, escalate_model: true });
        // Same failure after an applied fix → no progress.
        let same = HealState { signature: "s1", ..base(one) };
        assert!(matches!(decide(&same), HealDecision::Escalate(Escalation::NoProgress { .. })));
        // ... unless the user just granted more attempts.
        assert!(matches!(decide(&HealState { forced: true, ..same.clone() }), HealDecision::Attempt { .. }));
        // An agent that failed did not apply a fix: not "no progress".
        let failed: &'static [HealingAttempt] = Box::leak(Box::new([attempt("s2", HealingOutcome::AgentFailed)]));
        assert!(matches!(decide(&base(failed)), HealDecision::Attempt { attempt: 2, .. }));
        let two: &'static [HealingAttempt] = Box::leak(Box::new([
            attempt("s0", HealingOutcome::StillFailing),
            attempt("s1", HealingOutcome::StillFailing),
        ]));
        assert_eq!(decide(&base(two)), HealDecision::Escalate(Escalation::Attempts { used: 2, allowed: 2 }));
        assert!(matches!(decide(&HealState { granted: 2, ..base(two) }), HealDecision::Attempt { attempt: 3, .. }));
        assert_eq!(
            decide(&HealState { elapsed: Duration::from_secs(600), ..base(&[]) }),
            HealDecision::Escalate(Escalation::WallTime { mins: 10 })
        );
        assert!(matches!(
            decide(&HealState { spent: 1.5, ..base(&[]) }),
            HealDecision::Escalate(Escalation::Budget { .. })
        ));
        assert!(matches!(
            decide(&HealState { budget_exceeded: true, budget: None, ..base(&[]) }),
            HealDecision::Escalate(Escalation::Budget { .. })
        ));
        let off = HealingLimits { max_attempts: 0, ..HealingLimits::default() };
        assert_eq!(decide(&HealState { limits: &off, ..base(&[]) }), HealDecision::Escalate(Escalation::Disabled));
        assert!(Escalation::NoProgress { signature: "x".into() }.reason().contains("no progress"));
    }
}
