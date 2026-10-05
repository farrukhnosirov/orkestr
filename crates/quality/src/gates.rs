//! Quality gates and Definition of Done (§58) — pure evaluation.
//!
//! [`evaluate`] combines the latest [`CheckResult`]s per kind with review
//! state, security findings, acceptance criteria and the task's touched
//! files. It never runs anything; callers gather the inputs.

use std::collections::HashMap;

use orkestr_domain::{ProjectId, Task};
use orkestr_protocol::methods::quality::{
    CheckKind, CheckResult, CheckStatus, DodItem, DodResult, DodSetting, GateConfig, GateKind, GateReason, GateReport,
    GateResult, GateSetting, GateStatus,
};

use crate::sources::{ReviewStatus, SecurityStatus};

/// Default gate configuration: everything on and required except the
/// formatting check (advisory, off).
#[must_use]
pub fn default_config(project_id: ProjectId) -> GateConfig {
    GateConfig {
        project_id,
        gates: GateKind::ALL
            .iter()
            .map(|&gate| GateSetting {
                gate,
                enabled: gate != GateKind::FormatPasses,
                required: gate != GateKind::FormatPasses,
            })
            .collect(),
        dod: DodItem::ALL.iter().map(|&item| DodSetting { item, enabled: true }).collect(),
        customized: false,
        updated_at: None,
    }
}

/// Merges user settings over defaults: every gate / item exactly once, in
/// canonical order; unknown duplicates are ignored (first wins).
#[must_use]
pub fn normalize(project_id: ProjectId, gates: &[GateSetting], dod: &[DodSetting]) -> GateConfig {
    let mut cfg = default_config(project_id);
    for g in &mut cfg.gates {
        if let Some(s) = gates.iter().find(|s| s.gate == g.gate) {
            g.enabled = s.enabled;
            g.required = s.required;
        }
    }
    for d in &mut cfg.dod {
        if let Some(s) = dod.iter().find(|s| s.item == d.item) {
            d.enabled = s.enabled;
        }
    }
    cfg
}

/// Inputs of an evaluation.
#[derive(Debug, Clone)]
pub struct EvaluationInput<'a> {
    /// Project.
    pub project_id: ProjectId,
    /// Task (None = project-level).
    pub task: Option<&'a Task>,
    /// Gate configuration.
    pub config: &'a GateConfig,
    /// Kinds that have at least one enabled check in the profile.
    pub configured_kinds: &'a [CheckKind],
    /// Latest result per check id: `(run id, result)`, newest runs first.
    pub results: &'a [(String, CheckResult)],
    /// Latest run id considered.
    pub run_id: Option<String>,
    /// Review status.
    pub review: ReviewStatus,
    /// Security findings.
    pub security: SecurityStatus,
    /// Snapshots recorded for the task (rollback evidence).
    pub task_snapshots: usize,
}

fn gate_kinds(gate: GateKind) -> &'static [CheckKind] {
    match gate {
        GateKind::BuildPasses => &[CheckKind::Build],
        GateKind::TypecheckPasses => &[CheckKind::Typecheck],
        GateKind::LintPasses => &[CheckKind::Lint],
        GateKind::FormatPasses => &[CheckKind::Format],
        GateKind::TestsPass => &[CheckKind::Test, CheckKind::E2e, CheckKind::Smoke],
        _ => &[],
    }
}

/// Latest result per check id (first occurrence wins — inputs are newest first).
fn latest_by_check<'a>(results: &'a [(String, CheckResult)]) -> Vec<&'a (String, CheckResult)> {
    let mut seen: HashMap<&str, ()> = HashMap::new();
    let mut out = Vec::new();
    for r in results {
        if seen.insert(r.1.check_id.as_str(), ()).is_none() {
            out.push(r);
        }
    }
    out
}

fn check_gate(gate: GateKind, input: &EvaluationInput<'_>) -> (GateStatus, GateReason, String, Option<String>) {
    let kinds = gate_kinds(gate);
    if !kinds.iter().any(|k| input.configured_kinds.contains(k)) {
        return (GateStatus::Skipped, GateReason::NoCheckConfigured, String::new(), None);
    }
    let latest: Vec<_> = latest_by_check(input.results)
        .into_iter()
        .filter(|(_, r)| kinds.contains(&r.kind) && r.status != CheckStatus::Skipped)
        .collect();
    if latest.is_empty() {
        return (GateStatus::Pending, GateReason::NotRun, String::new(), None);
    }
    let run_id = latest.first().map(|(id, _)| id.clone());
    let labels = |pred: &dyn Fn(&CheckResult) -> bool| {
        latest.iter().filter(|(_, r)| pred(r)).map(|(_, r)| r.label.clone()).collect::<Vec<_>>().join(", ")
    };
    if latest.iter().any(|(_, r)| r.status == CheckStatus::Failed) {
        let failed: u32 = latest.iter().map(|(_, r)| r.failed).sum();
        let errors: usize = latest
            .iter()
            .map(|(_, r)| {
                r.diagnostics
                    .iter()
                    .filter(|d| d.severity == orkestr_protocol::methods::quality::DiagnosticSeverity::Error)
                    .count()
            })
            .sum();
        let detail = if kinds.iter().any(|k| k.is_test()) && failed > 0 {
            format!("{failed} failed — {}", labels(&|r| r.status == CheckStatus::Failed))
        } else if errors > 0 {
            format!("{errors} errors — {}", labels(&|r| r.status == CheckStatus::Failed))
        } else {
            labels(&|r| r.status == CheckStatus::Failed)
        };
        return (GateStatus::Failed, GateReason::CheckFailed, detail, run_id);
    }
    if latest.iter().any(|(_, r)| !r.status.is_final()) {
        return (GateStatus::Pending, GateReason::CheckRunning, labels(&|r| !r.status.is_final()), run_id);
    }
    if latest.iter().any(|(_, r)| r.status != CheckStatus::Passed) {
        return (
            GateStatus::Failed,
            GateReason::CheckIncomplete,
            labels(&|r| r.status != CheckStatus::Passed),
            run_id,
        );
    }
    let passed: u32 = latest.iter().map(|(_, r)| r.passed).sum();
    let detail = if kinds.iter().any(|k| k.is_test()) && passed > 0 {
        format!("{passed} passed")
    } else {
        labels(&|_| true)
    };
    (GateStatus::Passed, GateReason::CheckPassed, detail, run_id)
}

fn criteria(task: Option<&Task>) -> (GateStatus, GateReason, String) {
    let Some(task) = task else {
        return (GateStatus::Skipped, GateReason::NoTask, String::new());
    };
    let total = task.acceptance_criteria.len();
    if total == 0 {
        return (GateStatus::Unknown, GateReason::NoCriteria, String::new());
    }
    let met = task.acceptance_criteria.iter().filter(|c| c.met).count();
    let detail = format!("{met}/{total}");
    if met == total {
        (GateStatus::Passed, GateReason::CriteriaMet, detail)
    } else {
        (GateStatus::Pending, GateReason::CriteriaUnmet, detail)
    }
}

fn review(status: ReviewStatus) -> (GateStatus, GateReason) {
    match status {
        ReviewStatus::Approved => (GateStatus::Passed, GateReason::ReviewApproved),
        ReviewStatus::ChangesRequested | ReviewStatus::Rejected => (GateStatus::Failed, GateReason::ReviewRejected),
        ReviewStatus::Pending => (GateStatus::Pending, GateReason::ReviewPending),
        ReviewStatus::Unknown => (GateStatus::Unknown, GateReason::ReviewUnknown),
    }
}

fn security(status: SecurityStatus) -> (GateStatus, GateReason, String) {
    match status {
        SecurityStatus::Clean => (GateStatus::Passed, GateReason::SecurityClean, String::new()),
        SecurityStatus::Critical { count } => {
            (GateStatus::Failed, GateReason::SecurityCritical, count.to_string())
        }
        SecurityStatus::Unknown => (GateStatus::Unknown, GateReason::SecurityUnknown, String::new()),
    }
}

/// Evaluates every gate and the DoD checklist.
#[must_use]
pub fn evaluate(input: &EvaluationInput<'_>, now: orkestr_domain::Timestamp) -> GateReport {
    let mut gates = Vec::new();
    for setting in &input.config.gates {
        let (status, reason, detail, run_id) = if !setting.enabled {
            (GateStatus::Skipped, GateReason::Disabled, String::new(), None)
        } else {
            match setting.gate {
                GateKind::NoCriticalSecurity => {
                    let (s, r, d) = security(input.security);
                    (s, r, d, None)
                }
                GateKind::ReviewerApproved => {
                    let (s, r) = review(input.review);
                    (s, r, String::new(), None)
                }
                GateKind::AcceptanceCriteriaMet => {
                    let (s, r, d) = criteria(input.task);
                    (s, r, d, None)
                }
                g => check_gate(g, input),
            }
        };
        gates.push(GateResult {
            gate: setting.gate,
            enabled: setting.enabled,
            required: setting.required,
            status,
            reason,
            detail,
            run_id,
        });
    }
    let required: Vec<&GateResult> = gates.iter().filter(|g| g.enabled && g.required).collect();
    let overall = if required.iter().any(|g| g.status == GateStatus::Failed) {
        GateStatus::Failed
    } else if required.iter().all(|g| matches!(g.status, GateStatus::Passed | GateStatus::Skipped)) {
        GateStatus::Passed
    } else {
        GateStatus::Pending
    };
    let dod = input
        .config
        .dod
        .iter()
        .map(|s| {
            if s.enabled {
                dod_item(s.item, input, &gates)
            } else {
                DodResult { item: s.item, status: GateStatus::Skipped, reason: GateReason::Disabled, detail: String::new() }
            }
        })
        .collect();
    GateReport {
        project_id: input.project_id,
        task_id: input.task.map(|t| t.id),
        overall,
        gates,
        dod,
        run_id: input.run_id.clone(),
        evaluated_at: now,
    }
}

fn is_test_path(p: &str) -> bool {
    let l = p.to_ascii_lowercase().replace('\\', "/");
    let name = l.rsplit('/').next().unwrap_or(&l);
    l.split('/').any(|seg| seg == "tests" || seg == "test" || seg == "__tests__" || seg == "spec")
        || name.contains(".test.")
        || name.contains(".spec.")
        || name.contains("_test.")
        || name.starts_with("test_")
        || name.ends_with("tests.cs")
        || name.ends_with("test.java")
        || name.ends_with("tests.rs")
}

fn is_doc_path(p: &str) -> bool {
    let l = p.to_ascii_lowercase().replace('\\', "/");
    l.ends_with(".md") || l.ends_with(".mdx") || l.ends_with(".rst") || l.ends_with(".adoc") || l.starts_with("docs/")
        || l.contains("/docs/")
}

fn is_migration_path(p: &str) -> bool {
    let l = p.to_ascii_lowercase().replace('\\', "/");
    l.contains("migration") || l.contains("/migrate/") || l.ends_with(".sql") || l.contains("schema")
}

fn needs_migration(p: &str) -> bool {
    let l = p.to_ascii_lowercase();
    l.contains("model") || l.contains("entity") || l.contains("storage") || l.contains("/db/") || l.contains("database")
}

fn gate_status(gates: &[GateResult], g: GateKind) -> Option<&GateResult> {
    gates.iter().find(|x| x.gate == g && x.enabled)
}

fn dod_item(item: DodItem, input: &EvaluationInput<'_>, gates: &[GateResult]) -> DodResult {
    let touched: &[String] = input.task.map_or(&[], |t| t.files_touched.as_slice());
    let mk = |status, reason, detail: String| DodResult { item, status, reason, detail };
    match item {
        DodItem::Code => {
            if input.task.is_none() {
                return mk(GateStatus::Skipped, GateReason::NoTask, String::new());
            }
            let code = touched.iter().filter(|p| !is_doc_path(p)).count();
            if code > 0 {
                mk(GateStatus::Passed, GateReason::EvidenceFound, code.to_string())
            } else {
                mk(GateStatus::Pending, GateReason::NoEvidence, String::new())
            }
        }
        DodItem::Tests => {
            let tests_touched = touched.iter().filter(|p| is_test_path(p)).count();
            match gate_status(gates, GateKind::TestsPass).map(|g| g.status) {
                Some(GateStatus::Failed) => mk(GateStatus::Failed, GateReason::CheckFailed, String::new()),
                Some(GateStatus::Passed) if tests_touched > 0 || input.task.is_none() => {
                    mk(GateStatus::Passed, GateReason::EvidenceFound, tests_touched.to_string())
                }
                Some(GateStatus::Passed) => mk(GateStatus::Unknown, GateReason::NoEvidence, String::new()),
                _ if tests_touched > 0 => mk(GateStatus::Pending, GateReason::NotRun, tests_touched.to_string()),
                _ => mk(GateStatus::Pending, GateReason::NoEvidence, String::new()),
            }
        }
        DodItem::Docs => {
            if input.task.is_none() {
                return mk(GateStatus::Skipped, GateReason::NoTask, String::new());
            }
            let docs = touched.iter().filter(|p| is_doc_path(p)).count();
            if docs > 0 {
                mk(GateStatus::Passed, GateReason::EvidenceFound, docs.to_string())
            } else {
                mk(GateStatus::Unknown, GateReason::NoEvidence, String::new())
            }
        }
        DodItem::Migrations => {
            if input.task.is_none() {
                return mk(GateStatus::Skipped, GateReason::NoTask, String::new());
            }
            let migrations = touched.iter().filter(|p| is_migration_path(p)).count();
            if migrations > 0 {
                mk(GateStatus::Passed, GateReason::EvidenceFound, migrations.to_string())
            } else if touched.iter().any(|p| needs_migration(p)) {
                mk(GateStatus::Unknown, GateReason::NoEvidence, String::new())
            } else {
                mk(GateStatus::Skipped, GateReason::NotApplicable, String::new())
            }
        }
        DodItem::Rollback => {
            if input.task.is_none() {
                return mk(GateStatus::Skipped, GateReason::NoTask, String::new());
            }
            if input.task_snapshots > 0 {
                mk(GateStatus::Passed, GateReason::EvidenceFound, input.task_snapshots.to_string())
            } else if touched.is_empty() {
                mk(GateStatus::Skipped, GateReason::NotApplicable, String::new())
            } else {
                mk(GateStatus::Unknown, GateReason::NoEvidence, String::new())
            }
        }
        DodItem::Review => {
            let (s, r) = review(input.review);
            mk(s, r, String::new())
        }
        DodItem::AcceptanceCriteria => {
            let (s, r, d) = criteria(input.task);
            mk(s, r, d)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_domain::AcceptanceCriterion;
    use orkestr_protocol::methods::quality::Toolchain;

    fn result(id: &str, kind: CheckKind, status: CheckStatus, passed: u32, failed: u32) -> CheckResult {
        CheckResult {
            check_id: id.into(),
            kind,
            toolchain: Toolchain::Cargo,
            label: id.into(),
            command: id.into(),
            cwd: ".".into(),
            status,
            exit_code: Some(i32::from(status != CheckStatus::Passed)),
            duration_ms: 1,
            passed,
            failed,
            skipped: 0,
            failures: vec![],
            diagnostics: vec![],
            raw_tail: String::new(),
            summary: String::new(),
            approval_id: None,
            started_at: None,
        }
    }

    fn task(project: ProjectId) -> Task {
        let mut t = Task::new(project, "t", orkestr_domain::now());
        t.acceptance_criteria = vec![AcceptanceCriterion { text: "a".into(), met: true }];
        t
    }

    #[test]
    fn overall_passes_when_required_gates_pass() {
        let p = ProjectId::new();
        let mut cfg = default_config(p);
        // Review/security unknown would keep it pending: make them advisory.
        for g in &mut cfg.gates {
            if matches!(g.gate, GateKind::ReviewerApproved | GateKind::NoCriticalSecurity) {
                g.required = false;
            }
        }
        let t = task(p);
        let results = vec![
            ("r2".to_owned(), result("build", CheckKind::Build, CheckStatus::Passed, 0, 0)),
            ("r2".to_owned(), result("test", CheckKind::Test, CheckStatus::Passed, 5, 0)),
            ("r1".to_owned(), result("test", CheckKind::Test, CheckStatus::Failed, 4, 1)),
        ];
        let kinds = [CheckKind::Build, CheckKind::Test];
        let input = EvaluationInput {
            project_id: p,
            task: Some(&t),
            config: &cfg,
            configured_kinds: &kinds,
            results: &results,
            run_id: Some("r2".into()),
            review: ReviewStatus::Unknown,
            security: SecurityStatus::Unknown,
            task_snapshots: 0,
        };
        let r = evaluate(&input, orkestr_domain::now());
        let g = |k| r.gates.iter().find(|x| x.gate == k).unwrap().clone();
        assert_eq!(g(GateKind::BuildPasses).status, GateStatus::Passed);
        assert_eq!(g(GateKind::TestsPass).status, GateStatus::Passed, "latest result wins");
        assert_eq!(g(GateKind::TestsPass).detail, "5 passed");
        assert_eq!(g(GateKind::LintPasses).reason, GateReason::NoCheckConfigured);
        assert_eq!(g(GateKind::FormatPasses).reason, GateReason::Disabled);
        assert_eq!(g(GateKind::AcceptanceCriteriaMet).status, GateStatus::Passed);
        assert_eq!(g(GateKind::ReviewerApproved).status, GateStatus::Unknown);
        assert_eq!(r.overall, GateStatus::Passed);
    }

    #[test]
    fn failing_tests_fail_overall_and_dod() {
        let p = ProjectId::new();
        let cfg = default_config(p);
        let mut t = task(p);
        t.files_touched = vec!["src/lib.rs".into(), "tests/it.rs".into(), "README.md".into()];
        let results = vec![("r".to_owned(), result("test", CheckKind::Test, CheckStatus::Failed, 3, 2))];
        let kinds = [CheckKind::Test];
        let input = EvaluationInput {
            project_id: p,
            task: Some(&t),
            config: &cfg,
            configured_kinds: &kinds,
            results: &results,
            run_id: Some("r".into()),
            review: ReviewStatus::Approved,
            security: SecurityStatus::Critical { count: 1 },
            task_snapshots: 2,
        };
        let r = evaluate(&input, orkestr_domain::now());
        assert_eq!(r.overall, GateStatus::Failed);
        let tests = r.gates.iter().find(|x| x.gate == GateKind::TestsPass).unwrap();
        assert_eq!(tests.detail, "2 failed — test");
        assert_eq!(r.gates.iter().find(|x| x.gate == GateKind::NoCriticalSecurity).unwrap().status, GateStatus::Failed);
        let d = |i| r.dod.iter().find(|x| x.item == i).unwrap().status;
        assert_eq!(d(DodItem::Code), GateStatus::Passed);
        assert_eq!(d(DodItem::Tests), GateStatus::Failed);
        assert_eq!(d(DodItem::Docs), GateStatus::Passed);
        assert_eq!(d(DodItem::Migrations), GateStatus::Skipped);
        assert_eq!(d(DodItem::Rollback), GateStatus::Passed);
        assert_eq!(d(DodItem::Review), GateStatus::Passed);
    }

    #[test]
    fn pending_without_runs_and_incomplete_checks() {
        let p = ProjectId::new();
        let cfg = default_config(p);
        let kinds = [CheckKind::Build, CheckKind::Lint];
        let results = vec![("r".to_owned(), result("lint", CheckKind::Lint, CheckStatus::TimedOut, 0, 0))];
        let input = EvaluationInput {
            project_id: p,
            task: None,
            config: &cfg,
            configured_kinds: &kinds,
            results: &results,
            run_id: None,
            review: ReviewStatus::Unknown,
            security: SecurityStatus::Clean,
            task_snapshots: 0,
        };
        let r = evaluate(&input, orkestr_domain::now());
        let g = |k| r.gates.iter().find(|x| x.gate == k).unwrap().clone();
        assert_eq!(g(GateKind::BuildPasses).reason, GateReason::NotRun);
        assert_eq!(g(GateKind::LintPasses).reason, GateReason::CheckIncomplete);
        assert_eq!(g(GateKind::AcceptanceCriteriaMet).reason, GateReason::NoTask);
        assert_eq!(r.overall, GateStatus::Failed);
    }

    #[test]
    fn normalize_merges_and_orders() {
        let p = ProjectId::new();
        let cfg = normalize(
            p,
            &[GateSetting { gate: GateKind::LintPasses, enabled: false, required: false }],
            &[DodSetting { item: DodItem::Docs, enabled: false }],
        );
        assert_eq!(cfg.gates.len(), GateKind::ALL.len());
        assert!(!cfg.gates.iter().find(|g| g.gate == GateKind::LintPasses).unwrap().enabled);
        assert!(!cfg.dod.iter().find(|d| d.item == DodItem::Docs).unwrap().enabled);
        assert_eq!(cfg.gates[0].gate, GateKind::BuildPasses);
    }

    #[test]
    fn path_classifiers() {
        assert!(is_test_path("src/foo.test.ts"));
        assert!(is_test_path("tests/it.rs"));
        assert!(is_test_path("pkg/calc_test.go"));
        assert!(is_test_path("test_x.py"));
        assert!(!is_test_path("src/main.rs"));
        assert!(is_doc_path("docs/x.txt"));
        assert!(is_migration_path("crates/x/migrations/0001.sql"));
    }
}
