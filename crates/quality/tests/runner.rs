//! Runner tests: scripted executor (recorded tool outputs) + the real
//! terminal executor through a command gate.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use orkestr_domain::{ProjectId, ReviewState, Task};
use orkestr_events::{EventBus, Received};
use orkestr_protocol::methods::quality::{
    CheckDef, CheckKind, CheckSource, CheckStatus, GateKind, GateReason, GateStatus, RunTarget, Toolchain,
};
use orkestr_quality::{
    quality_migrations, CheckExecutor, ExecError, ExecOutput, ExecRequest, QualityConfig, QualityDeps, QualityError,
    QualityService, StartRun, TerminalExecutor,
};
use orkestr_storage::Db;
use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

const CARGO_TEST_FAIL: &str = "running 2 tests
test calc::adds ... ok
test calc::subs ... FAILED

failures:

---- calc::subs stdout ----

thread 'calc::subs' panicked at src/calc.rs:9:5:
assertion `left == right` failed

test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
";

const CLIPPY_FAIL: &str = "error: this looks like a formatting argument but it is not part of a formatting macro
 --> src/main.rs:2:15
error: could not compile `x` (bin \"x\") due to 1 previous error
";

enum Script {
    Out { code: i32, stdout: &'static str },
    Hang,
    Deny,
}

#[derive(Default)]
struct Scripted {
    scripts: Mutex<HashMap<String, Script>>,
    calls: Mutex<Vec<ExecRequest>>,
}

impl Scripted {
    fn with(entries: Vec<(&str, Script)>) -> Arc<Self> {
        let s = Self::default();
        for (k, v) in entries {
            s.scripts.lock().insert(k.to_owned(), v);
        }
        Arc::new(s)
    }
}

#[async_trait]
impl CheckExecutor for Scripted {
    async fn execute(&self, req: ExecRequest, cancel: CancellationToken) -> Result<ExecOutput, ExecError> {
        self.calls.lock().push(req.clone());
        let action = match self.scripts.lock().get(&req.command) {
            Some(Script::Out { code, stdout }) => Ok((*code, *stdout)),
            Some(Script::Hang) => Err(true),
            Some(Script::Deny) => Err(false),
            None => Ok((0, "")),
        };
        match action {
            Ok((code, stdout)) => Ok(ExecOutput {
                exit_code: Some(code),
                stdout: stdout.to_owned(),
                stderr: String::new(),
                timed_out: false,
                duration: Duration::from_millis(5),
            }),
            Err(true) => {
                tokio::select! {
                    () = cancel.cancelled() => Err(ExecError::Cancelled),
                    () = tokio::time::sleep(req.timeout) => Ok(ExecOutput {
                        exit_code: None, stdout: String::new(), stderr: String::new(), timed_out: true, duration: req.timeout,
                    }),
                }
            }
            Err(false) => {
                Err(ExecError::ApprovalRequired { reason: "dangerous".into(), approval_id: Some("ap1".into()) })
            }
        }
    }
}

fn check(id: &str, kind: CheckKind, command: &str) -> CheckDef {
    CheckDef {
        id: id.into(),
        kind,
        toolchain: Toolchain::Cargo,
        label: command.into(),
        command: command.into(),
        cwd: ".".into(),
        enabled: true,
        timeout_secs: None,
        source: CheckSource::User,
    }
}

async fn service(executor: Arc<dyn CheckExecutor>) -> (QualityService, EventBus) {
    let db = Db::open_in_memory().unwrap();
    db.migrate(quality_migrations()).await.unwrap();
    let events = EventBus::in_memory(1024);
    let svc = QualityService::new(QualityDeps {
        db,
        events: events.clone(),
        executor,
        redact: Arc::new(|s: &str| s.replace("hunter2", "[REDACTED]")),
        config: QualityConfig::default(),
    });
    svc.init().await.unwrap();
    (svc, events)
}

fn start(project_id: ProjectId, ws: &Path, checks: Vec<CheckDef>) -> StartRun {
    StartRun {
        project_id,
        task_id: None,
        agent_id: None,
        target: RunTarget::Project,
        workspace: ws.to_path_buf(),
        checks,
        kinds: vec![],
        check_ids: vec![],
        budget: Duration::from_secs(60),
    }
}

#[tokio::test]
async fn run_parses_results_publishes_events_and_persists() {
    let exec = Scripted::with(vec![
        ("cargo build", Script::Out { code: 0, stdout: "Finished dev" }),
        ("cargo clippy", Script::Out { code: 101, stdout: CLIPPY_FAIL }),
        ("cargo test", Script::Out { code: 101, stdout: CARGO_TEST_FAIL }),
    ]);
    let (svc, events) = service(exec.clone()).await;
    let mut sub = events.subscribe_all();
    let ws = tempfile::tempdir().unwrap();
    let project = ProjectId::new();
    let mut task = Task::new(project, "t", orkestr_domain::now());
    task.review_state = ReviewState::Approved;
    let mut req = start(
        project,
        ws.path(),
        vec![
            check("test", CheckKind::Test, "cargo test"),
            check("lint", CheckKind::Lint, "cargo clippy"),
            check("build", CheckKind::Build, "cargo build"),
        ],
    );
    req.task_id = Some(task.id);
    let initial = svc.start(req).await.unwrap();
    assert_eq!(initial.status, CheckStatus::Running);
    assert_eq!(
        initial.results.iter().map(|r| r.kind).collect::<Vec<_>>(),
        vec![CheckKind::Build, CheckKind::Lint, CheckKind::Test]
    );
    let run = svc.wait(&initial.id).await.unwrap();
    assert_eq!(run.status, CheckStatus::Failed);
    let build = &run.results[0];
    assert_eq!(build.status, CheckStatus::Passed);
    let lint = &run.results[1];
    assert_eq!(lint.status, CheckStatus::Failed);
    assert_eq!(lint.diagnostics.len(), 1);
    assert_eq!(lint.diagnostics[0].file.as_deref(), Some("src/main.rs"));
    let test = &run.results[2];
    assert_eq!((test.passed, test.failed), (1, 1));
    assert_eq!(test.failures[0].name, "calc::subs");
    assert_eq!(test.failures[0].line, Some(9));
    assert!(test.raw_tail.contains("test result: FAILED"));
    assert_eq!(test.summary, "1 passed, 1 failed, 0 skipped");

    // Persisted and listed for the task.
    let listed = svc.runs(project, Some(task.id), 10).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0], run);

    // Events: 3 started + build succeeded + build failed (lint) + test finished.
    let mut kinds = Vec::new();
    while kinds.len() < 6 {
        match tokio::time::timeout(Duration::from_secs(2), sub.recv()).await.unwrap().unwrap() {
            Received::Event(e) => {
                assert_eq!(e.project_id, Some(project));
                assert_eq!(e.task_id, Some(task.id));
                kinds.push(e.kind.name().to_owned());
            }
            Received::Lagged(_) => panic!("lagged"),
        }
    }
    assert_eq!(
        kinds,
        vec!["BuildStarted", "BuildSucceeded", "BuildStarted", "BuildFailed", "BuildStarted", "TestRunFinished"]
    );

    // Gates: build passes, lint + tests fail, review approved (task field).
    let report =
        svc.evaluate(project, Some(&task), &[CheckKind::Build, CheckKind::Lint, CheckKind::Test], 0).await.unwrap();
    let g = |k: GateKind| report.gates.iter().find(|x| x.gate == k).unwrap().clone();
    assert_eq!(g(GateKind::BuildPasses).status, GateStatus::Passed);
    assert_eq!(g(GateKind::LintPasses).status, GateStatus::Failed);
    assert_eq!(g(GateKind::TestsPass).status, GateStatus::Failed);
    assert_eq!(g(GateKind::ReviewerApproved).status, GateStatus::Passed);
    assert_eq!(g(GateKind::NoCriticalSecurity).reason, GateReason::SecurityUnknown);
    assert_eq!(report.overall, GateStatus::Failed);
    assert_eq!(report.run_id.as_deref(), Some(run.id.as_str()));
}

#[tokio::test]
async fn failed_build_skips_tests_and_redacts_output() {
    let exec = Scripted::with(vec![("cargo build", Script::Out { code: 1, stdout: "error: token hunter2 leaked" })]);
    let (svc, _) = service(exec.clone()).await;
    let ws = tempfile::tempdir().unwrap();
    let run = svc
        .start(start(
            ProjectId::new(),
            ws.path(),
            vec![check("b", CheckKind::Build, "cargo build"), check("t", CheckKind::Test, "cargo test")],
        ))
        .await
        .unwrap();
    let run = svc.wait(&run.id).await.unwrap();
    assert_eq!(run.results[0].status, CheckStatus::Failed);
    assert!(run.results[0].raw_tail.contains("[REDACTED]"));
    assert!(!run.results[0].raw_tail.contains("hunter2"));
    assert!(run.results[0].diagnostics.iter().all(|d| !d.message.contains("hunter2")));
    assert_eq!(run.results[1].status, CheckStatus::Skipped);
    assert_eq!(exec.calls.lock().len(), 1, "tests were not executed");
}

#[tokio::test]
async fn cancel_and_busy_workspace() {
    let exec = Scripted::with(vec![("hang", Script::Hang)]);
    let (svc, _) = service(exec).await;
    let ws = tempfile::tempdir().unwrap();
    let project = ProjectId::new();
    let run = svc.start(start(project, ws.path(), vec![check("h", CheckKind::Test, "hang")])).await.unwrap();
    // Same workspace → busy.
    let busy = svc.start(start(project, ws.path(), vec![check("h", CheckKind::Test, "hang")])).await;
    assert!(matches!(busy, Err(QualityError::Busy(_))));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(svc.run(&run.id).await.unwrap().results[0].status, CheckStatus::Running);
    assert!(svc.cancel(&run.id));
    let done = tokio::time::timeout(Duration::from_secs(5), svc.wait(&run.id)).await.unwrap().unwrap();
    assert_eq!(done.status, CheckStatus::Cancelled);
    assert_eq!(done.results[0].status, CheckStatus::Cancelled);
    assert!(!svc.cancel(&run.id), "no longer active");
    // Workspace free again.
    let again = svc.start(start(project, ws.path(), vec![check("x", CheckKind::Build, "ok")])).await.unwrap();
    assert_eq!(svc.wait(&again.id).await.unwrap().status, CheckStatus::Passed);
}

#[tokio::test]
async fn budget_times_out_and_skips_remaining() {
    let exec = Scripted::with(vec![("hang", Script::Hang)]);
    let (svc, _) = service(exec).await;
    let ws = tempfile::tempdir().unwrap();
    let mut req = start(
        ProjectId::new(),
        ws.path(),
        vec![check("h", CheckKind::Lint, "hang"), check("t", CheckKind::Test, "next")],
    );
    req.budget = Duration::from_secs(1);
    let run = svc.start(req).await.unwrap();
    let done = tokio::time::timeout(Duration::from_secs(10), svc.wait(&run.id)).await.unwrap().unwrap();
    assert_eq!(done.results[0].status, CheckStatus::TimedOut);
    assert_eq!(done.results[1].status, CheckStatus::Skipped);
    assert_eq!(done.status, CheckStatus::Failed);
}

#[tokio::test]
async fn approval_required_blocks_check() {
    let exec = Scripted::with(vec![("rm -rf x", Script::Deny)]);
    let (svc, _) = service(exec).await;
    let ws = tempfile::tempdir().unwrap();
    let run =
        svc.start(start(ProjectId::new(), ws.path(), vec![check("d", CheckKind::Smoke, "rm -rf x")])).await.unwrap();
    let done = svc.wait(&run.id).await.unwrap();
    assert_eq!(done.results[0].status, CheckStatus::Blocked);
    assert_eq!(done.results[0].approval_id.as_deref(), Some("ap1"));
    assert_eq!(done.status, CheckStatus::Failed);
}

#[tokio::test]
async fn nothing_to_run_and_invalid_workspace() {
    let (svc, _) = service(Scripted::with(vec![])).await;
    let ws = tempfile::tempdir().unwrap();
    let mut c = check("b", CheckKind::Build, "x");
    c.enabled = false;
    let r = svc.start(start(ProjectId::new(), ws.path(), vec![c])).await;
    assert!(matches!(r, Err(QualityError::NothingToRun(_))));
    let r =
        svc.start(start(ProjectId::new(), &ws.path().join("missing"), vec![check("b", CheckKind::Build, "x")])).await;
    assert!(matches!(r, Err(QualityError::Invalid(_))));
}

#[tokio::test]
async fn profile_detection_overrides_and_gates() {
    let (svc, _) = service(Scripted::with(vec![])).await;
    let ws = tempfile::tempdir().unwrap();
    std::fs::write(ws.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
    let project = ProjectId::new();
    let p = svc.profile(project, Some(ws.path())).await.unwrap();
    assert!(!p.customized);
    assert!(p.checks.iter().any(|c| c.command == "cargo test"));
    let mut checks = p.checks.clone();
    checks.push(check("custom", CheckKind::Smoke, "  ./smoke.sh  "));
    let saved = svc.set_profile(project, checks, false, Some(ws.path())).await.unwrap();
    assert!(saved.customized);
    assert_eq!(saved.checks.last().unwrap().command, "./smoke.sh");
    let again = svc.profile(project, Some(ws.path())).await.unwrap();
    assert_eq!(again.checks, saved.checks);
    let bad = svc.set_profile(project, vec![check("a", CheckKind::Build, "")], false, None).await;
    assert!(matches!(bad, Err(QualityError::Invalid(_))));
    let reset = svc.set_profile(project, vec![], true, Some(ws.path())).await.unwrap();
    assert!(!reset.customized);

    let cfg = svc.gates(project).await.unwrap();
    assert!(!cfg.customized);
    let mut lint = cfg.gates.iter().find(|g| g.gate == GateKind::LintPasses).unwrap().clone();
    lint.enabled = false;
    let saved = svc.set_gates(project, &[lint], &[], false).await.unwrap();
    assert!(saved.customized);
    assert!(!saved.gates.iter().find(|g| g.gate == GateKind::LintPasses).unwrap().enabled);
    assert!(saved.gates.iter().find(|g| g.gate == GateKind::BuildPasses).unwrap().enabled, "others kept");
    assert!(!svc.set_gates(project, &[], &[], true).await.unwrap().customized);
}

/// The real terminal executor: commands run through the terminal service and
/// its command gate.
#[tokio::test]
async fn terminal_executor_runs_real_commands_through_the_gate() {
    let terminals = orkestr_terminal::TerminalService::builder().gate(Arc::new(orkestr_terminal::AllowAllGate)).build();
    let (svc, _) = service(Arc::new(TerminalExecutor::new(terminals.clone()))).await;
    let ws = tempfile::tempdir().unwrap();
    let run = svc
        .start(start(
            ProjectId::new(),
            ws.path(),
            vec![check("ok", CheckKind::Build, "exit 0"), check("bad", CheckKind::Lint, "exit 3")],
        ))
        .await
        .unwrap();
    let done = tokio::time::timeout(Duration::from_secs(60), svc.wait(&run.id)).await.unwrap().unwrap();
    assert_eq!(done.results[0].status, CheckStatus::Passed, "{:?}", done.results[0]);
    assert_eq!(done.results[1].status, CheckStatus::Failed);
    assert_eq!(done.results[1].exit_code, Some(3));
    assert_eq!(done.results[1].summary, "exit code 3");

    // Agent-owned commands are gated: the default user-only gate refuses them.
    let gated = orkestr_terminal::TerminalService::builder().build();
    let (svc, _) = service(Arc::new(TerminalExecutor::new(gated))).await;
    let mut req = start(ProjectId::new(), ws.path(), vec![check("ok", CheckKind::Build, "exit 0")]);
    req.agent_id = Some(orkestr_domain::AgentId::new());
    let run = svc.start(req).await.unwrap();
    let done = svc.wait(&run.id).await.unwrap();
    assert_eq!(done.results[0].status, CheckStatus::Blocked, "{:?}", done.results[0]);
    terminals.shutdown().await;
}

/// Records gate requests (shell family + command).
#[derive(Default)]
struct RecordingGate(Mutex<Vec<(orkestr_terminal::ShellKind, String)>>);

#[async_trait]
impl orkestr_terminal::CommandGate for RecordingGate {
    async fn check(&self, request: &orkestr_terminal::GateRequest) -> orkestr_terminal::GateDecision {
        self.0.lock().push((request.shell, request.command.clone()));
        orkestr_terminal::GateDecision::Allow
    }
}

/// Checks run in the platform command shell (`cmd.exe` on Windows) and the
/// gate classifies the detected command itself, not a wrapper.
#[tokio::test]
async fn terminal_executor_uses_command_shell_and_gates_inner_command() {
    let gate = Arc::new(RecordingGate::default());
    let terminals = orkestr_terminal::TerminalService::builder().gate(gate.clone()).build();
    let (svc, _) = service(Arc::new(TerminalExecutor::new(terminals.clone()))).await;
    let ws = tempfile::tempdir().unwrap();
    let run =
        svc.start(start(ProjectId::new(), ws.path(), vec![check("ok", CheckKind::Build, "echo built")])).await.unwrap();
    let done = tokio::time::timeout(Duration::from_secs(60), svc.wait(&run.id)).await.unwrap().unwrap();
    assert_eq!(done.results[0].status, CheckStatus::Passed, "{:?}", done.results[0]);
    let seen = gate.0.lock().clone();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].1, "echo built");
    if cfg!(windows) {
        assert_eq!(seen[0].0, orkestr_terminal::ShellKind::Cmd);
    }
    terminals.shutdown().await;
}

/// Regression (Phase 2 acceptance): on Windows `npm …` checks failed before
/// running because PowerShell's default execution policy refuses `npm.ps1`.
/// Skipped when npm is not installed.
#[cfg(windows)]
#[tokio::test]
async fn windows_npm_check_runs_without_execution_policy_error() {
    if orkestr_platform::process::which("npm.cmd").is_none() {
        eprintln!("npm not installed; skipping");
        return;
    }
    let terminals = orkestr_terminal::TerminalService::builder().gate(Arc::new(orkestr_terminal::AllowAllGate)).build();
    let (svc, _) = service(Arc::new(TerminalExecutor::new(terminals.clone()))).await;
    let ws = tempfile::tempdir().unwrap();
    let run = svc
        .start(start(ProjectId::new(), ws.path(), vec![check("npm", CheckKind::Build, "npm --version")]))
        .await
        .unwrap();
    let done = tokio::time::timeout(Duration::from_secs(120), svc.wait(&run.id)).await.unwrap().unwrap();
    let r = &done.results[0];
    assert_eq!(r.status, CheckStatus::Passed, "{r:?}");
    assert_eq!(r.exit_code, Some(0), "{r:?}");
    terminals.shutdown().await;
}
