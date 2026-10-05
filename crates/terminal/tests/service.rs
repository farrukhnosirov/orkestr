//! Real-process tests of the terminal service (PTY sessions and `run`).
//! Windows uses `cmd`, other OSes `sh`.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use orkestr_domain::{AgentId, ProjectId, TerminalId};
use orkestr_terminal::{
    AllowAllGate, ChannelSink, CommandGate, EnvVar, GateDecision, GateRequest, OpenRequest, RunRequest, ShellSpec,
    SinkEvent, TerminalError, TerminalOwner, TerminalService, TerminalStatus,
};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_util::sync::CancellationToken;

#[cfg(windows)]
const SHELL: &str = "cmd";
#[cfg(not(windows))]
const SHELL: &str = "sh";

/// Shell-specific snippets.
#[cfg(windows)]
mod sh {
    pub const SIX_TIMES_SEVEN: &str = "set /a 6*7";
    pub const EXIT_7: &str = "exit 7";
    pub fn sleep_then_touch(secs: u32, marker: &str) -> String {
        // Nested cmd so the marker writer is a grandchild of the spawned shell.
        format!("cmd /d /c \"ping -n {} 127.0.0.1 >nul & echo x > \"{marker}\"\"", secs + 1)
    }
    pub fn print_env(name: &str) -> String {
        format!("echo [%{name}%]")
    }
    pub fn unset_env_echo(name: &str) -> String {
        format!("[%{name}%]")
    }
    pub const LOTS_OF_OUTPUT: &str = "for /L %i in (1,1,400) do @echo 0123456789";
    pub const STDERR: &str = "echo oops 1>&2";
}

#[cfg(not(windows))]
mod sh {
    pub const SIX_TIMES_SEVEN: &str = "echo $((6*7))";
    pub const EXIT_7: &str = "exit 7";
    pub fn sleep_then_touch(secs: u32, marker: &str) -> String {
        format!("sh -c 'sleep {secs}; echo x > \"{marker}\"'")
    }
    pub fn print_env(name: &str) -> String {
        format!("echo \"[${name}]\"")
    }
    pub fn unset_env_echo(_name: &str) -> String {
        "[]".into()
    }
    pub const LOTS_OF_OUTPUT: &str = "i=0; while [ $i -lt 400 ]; do echo 0123456789; i=$((i+1)); done";
    pub const STDERR: &str = "echo oops 1>&2";
}

fn service(gate: Arc<dyn CommandGate>) -> (TerminalService, UnboundedReceiver<SinkEvent>) {
    let (sink, rx) = ChannelSink::new();
    let svc = TerminalService::builder().gate(gate).sink(Arc::new(sink)).build();
    (svc, rx)
}

fn run_req(root: &Path, command: &str) -> RunRequest {
    RunRequest {
        root: Some(root.to_path_buf()),
        shell: ShellSpec::Id(SHELL.into()),
        command: command.into(),
        stream_to_view: Some(false),
        ..RunRequest::default()
    }
}

/// Collects output of `id` until `pred(all_output)` or timeout; returns (output, exit).
async fn collect_until(
    rx: &mut UnboundedReceiver<SinkEvent>,
    id: TerminalId,
    timeout: Duration,
    mut pred: impl FnMut(&str, Option<Option<i32>>) -> bool,
) -> (String, Option<Option<i32>>) {
    let mut out = String::new();
    let mut exit = None;
    let deadline = tokio::time::Instant::now() + timeout;
    while !pred(&out, exit) {
        let ev = tokio::time::timeout_at(deadline, rx.recv()).await;
        match ev {
            Ok(Some(SinkEvent::Output { terminal_id, data, .. })) if terminal_id == id => out.push_str(&data),
            Ok(Some(SinkEvent::Exited { terminal_id, exit_code })) if terminal_id == id => exit = Some(exit_code),
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(_) => panic!("timeout waiting for terminal output; got so far: {out:?}"),
        }
    }
    (out, exit)
}

struct DenyContaining(&'static str);

#[async_trait]
impl CommandGate for DenyContaining {
    async fn check(&self, request: &GateRequest) -> GateDecision {
        if request.command.contains(self.0) {
            GateDecision::Deny { reason: format!("`{}` is not allowed", self.0) }
        } else {
            GateDecision::Allow
        }
    }
}

struct AskAll;

#[async_trait]
impl CommandGate for AskAll {
    async fn check(&self, _request: &GateRequest) -> GateDecision {
        GateDecision::Ask { reason: "needs approval".into(), approval_id: Some("appr-1".into()) }
    }
}

// ---------------------------------------------------------------------- run

#[tokio::test]
async fn run_echo_and_exit_codes() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, _rx) = service(Arc::new(AllowAllGate));
    let out = svc.run(run_req(dir.path(), "echo hello"), CancellationToken::new()).await.unwrap();
    assert_eq!(out.exit_code, Some(0));
    assert_eq!(out.stdout.trim(), "hello");
    assert!(!out.truncated && !out.timed_out);

    let out = svc.run(run_req(dir.path(), sh::EXIT_7), CancellationToken::new()).await.unwrap();
    assert_eq!(out.exit_code, Some(7));

    let out = svc.run(run_req(dir.path(), sh::STDERR), CancellationToken::new()).await.unwrap();
    assert_eq!(out.stderr.trim(), "oops");
    assert_eq!(out.stdout.trim(), "");
}

#[tokio::test]
async fn run_utf8_output_roundtrips() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, _rx) = service(Arc::new(AllowAllGate));
    // cmd's builtins write the OEM code page to pipes; PowerShell is set to UTF-8.
    #[cfg(windows)]
    let req = RunRequest {
        shell: ShellSpec::Id("powershell".into()),
        ..run_req(dir.path(), "Write-Output 'Привет-世界'")
    };
    #[cfg(not(windows))]
    let req = run_req(dir.path(), "echo Привет-世界");
    let out = svc.run(req, CancellationToken::new()).await.unwrap();
    assert_eq!(out.exit_code, Some(0), "{out:?}");
    assert_eq!(out.stdout.trim(), "Привет-世界");
}

#[tokio::test]
async fn run_timeout_kills_process_tree() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("marker.txt");
    let (svc, _rx) = service(Arc::new(AllowAllGate));
    let mut req = run_req(dir.path(), &sh::sleep_then_touch(3, &marker.to_string_lossy()));
    req.timeout = Some(Duration::from_millis(500));
    let started = std::time::Instant::now();
    let out = svc.run(req, CancellationToken::new()).await.unwrap();
    assert!(out.timed_out, "{out:?}");
    assert_eq!(out.exit_code, None);
    assert!(started.elapsed() < Duration::from_secs(10), "took {:?}", started.elapsed());
    // The grandchild would write the marker after ~3s if it survived.
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(!marker.exists(), "grandchild survived the tree kill");
}

#[tokio::test]
async fn run_cancel_kills_and_reports_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("cancel-marker.txt");
    let (svc, _rx) = service(Arc::new(AllowAllGate));
    let token = CancellationToken::new();
    let t2 = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(400)).await;
        t2.cancel();
    });
    let started = std::time::Instant::now();
    let res = svc.run(run_req(dir.path(), &sh::sleep_then_touch(3, &marker.to_string_lossy())), token).await;
    assert!(matches!(res, Err(TerminalError::Cancelled)), "{res:?}");
    assert!(started.elapsed() < Duration::from_secs(10));
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(!marker.exists(), "cancelled command kept running");
}

#[tokio::test]
async fn run_stop_agent_cancels_its_commands() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, _rx) = service(Arc::new(AllowAllGate));
    let agent = AgentId::new();
    let mut req = run_req(dir.path(), &sh::sleep_then_touch(5, &dir.path().join("m").to_string_lossy()));
    req.owner = TerminalOwner::Agent { agent_id: agent };
    let svc2 = svc.clone();
    let handle = tokio::spawn(async move { svc2.run(req, CancellationToken::new()).await });
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(svc.stop_agent(agent).await >= 1);
    let res = tokio::time::timeout(Duration::from_secs(10), handle).await.unwrap().unwrap();
    assert!(matches!(res, Err(TerminalError::Cancelled)), "{res:?}");
}

#[tokio::test]
async fn run_gate_deny_never_spawns() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("denied.txt");
    let (svc, mut rx) = service(Arc::new(DenyContaining("denied.txt")));
    let mut req = run_req(dir.path(), &format!("echo x > \"{}\"", marker.display()));
    req.owner = TerminalOwner::Agent { agent_id: AgentId::new() };
    req.stream_to_view = None; // agents stream to a view by default
    let res = svc.run(req, CancellationToken::new()).await;
    assert!(matches!(res, Err(TerminalError::Denied(_))), "{res:?}");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!marker.exists());
    // The denial is visible in the agent's command view.
    let views = svc.list(None);
    assert_eq!(views.len(), 1);
    let (out, _) =
        collect_until(&mut rx, views[0].id, Duration::from_secs(5), |o, _| o.contains("command blocked")).await;
    assert!(out.contains("denied.txt"));
}

#[tokio::test]
async fn run_gate_ask_surfaces_approval() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, _rx) = service(Arc::new(AskAll));
    let res = svc.run(run_req(dir.path(), "echo hi"), CancellationToken::new()).await;
    match res {
        Err(TerminalError::ApprovalRequired { approval_id, .. }) => assert_eq!(approval_id.as_deref(), Some("appr-1")),
        other => panic!("expected approval, got {other:?}"),
    }
}

#[tokio::test]
async fn default_gate_requires_approval_for_agents_only() {
    let dir = tempfile::tempdir().unwrap();
    let svc = TerminalService::builder().build();
    assert!(svc.run(run_req(dir.path(), "echo ok"), CancellationToken::new()).await.is_ok());
    let mut req = run_req(dir.path(), "echo ok");
    req.owner = TerminalOwner::Agent { agent_id: AgentId::new() };
    assert!(matches!(svc.run(req, CancellationToken::new()).await, Err(TerminalError::ApprovalRequired { .. })));
}

#[tokio::test]
async fn run_truncates_output_and_sanitizes_env() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, _rx) = service(Arc::new(AllowAllGate));
    let mut req = run_req(dir.path(), sh::LOTS_OF_OUTPUT);
    req.max_output = Some(100);
    let out = svc.run(req, CancellationToken::new()).await.unwrap();
    assert!(out.truncated);
    assert!(out.stdout.len() <= 100);

    std::env::set_var("ORKESTR_TEST_LEAK_TOKEN", "must-not-leak");
    let out = svc
        .run(run_req(dir.path(), &sh::print_env("ORKESTR_TEST_LEAK_TOKEN")), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out.stdout.trim(), sh::unset_env_echo("ORKESTR_TEST_LEAK_TOKEN"));
    let mut req = run_req(dir.path(), &sh::print_env("INJECTED_VALUE"));
    req.env = vec![EnvVar::new("INJECTED_VALUE", "yes")];
    let out = svc.run(req, CancellationToken::new()).await.unwrap();
    assert_eq!(out.stdout.trim(), "[yes]");
}

#[tokio::test]
async fn run_confines_cwd_and_requires_root_for_agents() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    std::fs::create_dir_all(root.join("sub")).unwrap();
    let (svc, _rx) = service(Arc::new(AllowAllGate));
    let mut req = run_req(&root, "echo hi");
    req.cwd = Some("..".into());
    assert!(matches!(svc.run(req, CancellationToken::new()).await, Err(TerminalError::CwdOutsideRoot { .. })));

    let mut req = run_req(&root, "echo hi");
    req.root = None;
    req.owner = TerminalOwner::Agent { agent_id: AgentId::new() };
    assert!(matches!(svc.run(req, CancellationToken::new()).await, Err(TerminalError::Forbidden(_))));

    let mut req = run_req(&root, "cd");
    req.cwd = Some("sub".into());
    #[cfg(not(windows))]
    {
        req.command = "pwd".into();
    }
    let out = svc.run(req, CancellationToken::new()).await.unwrap();
    assert!(out.stdout.trim().ends_with("sub"), "{out:?}");
}

#[tokio::test]
async fn run_streams_to_agent_view() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, mut rx) = service(Arc::new(AllowAllGate));
    let agent = AgentId::new();
    let project = ProjectId::new();
    let mut req = run_req(dir.path(), "echo streamed-line");
    req.owner = TerminalOwner::Agent { agent_id: agent };
    req.project_id = Some(project);
    req.stream_to_view = None;
    let out = svc.run(req.clone(), CancellationToken::new()).await.unwrap();
    let view = out.view_terminal_id.expect("agent output is streamed to a view");
    let (text, _) = collect_until(&mut rx, view, Duration::from_secs(5), |o, _| o.contains("[exit 0")).await;
    assert!(text.contains("$ echo streamed-line"));
    assert!(text.contains("streamed-line\r\n"));
    // Second run reuses the same view.
    let out2 = svc.run(req, CancellationToken::new()).await.unwrap();
    assert_eq!(out2.view_terminal_id, Some(view));
    let info = svc.info(view).unwrap();
    assert_eq!(info.owner, TerminalOwner::Agent { agent_id: agent });
    assert!(matches!(svc.write(view, TerminalOwner::User, "x").await, Err(TerminalError::Forbidden(_))));
    let hist = svc.history(view, None).unwrap();
    assert!(hist.data.contains("streamed-line"));
    svc.close(view, TerminalOwner::User).await.unwrap();
    assert!(svc.list(None).is_empty());
}

// ---------------------------------------------------------------------- PTY

fn open_req(root: &Path) -> OpenRequest {
    OpenRequest {
        root: Some(root.to_path_buf()),
        shell: ShellSpec::Id(SHELL.into()),
        cols: 100,
        rows: 30,
        ..OpenRequest::default()
    }
}

#[tokio::test]
async fn pty_session_write_resize_history_exit() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, mut rx) = service(Arc::new(AllowAllGate));
    let info = svc.open(open_req(dir.path())).await.unwrap();
    assert_eq!(info.status, TerminalStatus::Running);
    assert!(info.pid.is_some());
    svc.resize(info.id, 132, 40).unwrap();
    assert_eq!(svc.info(info.id).unwrap().cols, 132);
    assert!(svc.resize(info.id, 0, 40).is_err());

    svc.write(info.id, TerminalOwner::User, &format!("{}\r", sh::SIX_TIMES_SEVEN)).await.unwrap();
    let (out, _) = collect_until(&mut rx, info.id, Duration::from_secs(20), |o, _| o.contains("42")).await;
    assert!(out.contains("42"));
    let hist = svc.history(info.id, None).unwrap();
    assert!(hist.data.contains("42"));
    assert_eq!(hist.end_offset as usize, hist.start_offset as usize + hist.data.len());

    svc.write(info.id, TerminalOwner::User, &format!("{}\r", sh::EXIT_7)).await.unwrap();
    let (_, exit) = collect_until(&mut rx, info.id, Duration::from_secs(20), |_, e| e.is_some()).await;
    assert_eq!(exit, Some(Some(7)));
    assert_eq!(svc.info(info.id).unwrap().status, TerminalStatus::Exited { exit_code: Some(7) });
    assert!(matches!(svc.write(info.id, TerminalOwner::User, "x").await, Err(TerminalError::Exited(_))));
    svc.close(info.id, TerminalOwner::User).await.unwrap();
    assert!(matches!(svc.info(info.id), Err(TerminalError::NotFound(_))));
}

#[tokio::test]
async fn pty_close_kills_running_shell_and_emits_exit() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, mut rx) = service(Arc::new(AllowAllGate));
    let info = svc.open(open_req(dir.path())).await.unwrap();
    let pid = info.pid.unwrap();
    svc.close(info.id, TerminalOwner::User).await.unwrap();
    let (_, exit) = collect_until(&mut rx, info.id, Duration::from_secs(15), |_, e| e.is_some()).await;
    assert!(exit.is_some());
    tokio::time::sleep(Duration::from_millis(500)).await;
    use orkestr_platform::ProcessControl;
    assert!(!orkestr_platform::NativeProcessControl.is_alive(pid), "shell pid {pid} still alive");
}

#[tokio::test]
async fn agent_pty_is_read_only_for_users_and_gated_per_line() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, mut rx) = service(Arc::new(DenyContaining("forbidden-cmd")));
    let agent = AgentId::new();
    let mut req = open_req(dir.path());
    req.owner = TerminalOwner::Agent { agent_id: agent };
    let info = svc.open(req).await.unwrap();

    assert!(matches!(svc.write(info.id, TerminalOwner::User, "dir\r").await, Err(TerminalError::Forbidden(_))));
    let other = TerminalOwner::Agent { agent_id: AgentId::new() };
    assert!(matches!(svc.write(info.id, other, "dir\r").await, Err(TerminalError::Forbidden(_))));
    assert!(matches!(svc.close(info.id, other).await, Err(TerminalError::Forbidden(_))));

    svc.exec_line(info.id, sh::SIX_TIMES_SEVEN).await.unwrap();
    let (out, _) = collect_until(&mut rx, info.id, Duration::from_secs(20), |o, _| o.contains("42")).await;
    assert!(out.contains("42"));

    let res = svc.exec_line(info.id, "forbidden-cmd --now").await;
    assert!(matches!(res, Err(TerminalError::Denied(_))), "{res:?}");
    let (_, _) = collect_until(&mut rx, info.id, Duration::from_secs(10), |o, _| o.contains("command blocked")).await;
    // Escape sequences from agents are rejected.
    assert!(matches!(
        svc.write(info.id, TerminalOwner::Agent { agent_id: agent }, "\u{1b}[A").await,
        Err(TerminalError::Forbidden(_))
    ));

    assert_eq!(svc.stop_agent(agent).await, 1);
    let (_, exit) = collect_until(&mut rx, info.id, Duration::from_secs(15), |_, e| e.is_some()).await;
    assert!(exit.is_some());
}

#[tokio::test]
async fn session_limit_and_shutdown() {
    let dir = tempfile::tempdir().unwrap();
    let (sink, _rx) = ChannelSink::new();
    let svc = TerminalService::builder()
        .sink(Arc::new(sink))
        .config(orkestr_terminal::TerminalConfig { max_sessions: 1, ..Default::default() })
        .build();
    let a = svc.open(open_req(dir.path())).await.unwrap();
    assert!(matches!(svc.open(open_req(dir.path())).await, Err(TerminalError::LimitReached(1))));
    svc.rename(a.id, "  Build  ").unwrap();
    assert_eq!(svc.info(a.id).unwrap().title, "Build");
    assert!(svc.rename(a.id, "").is_err());
    svc.shutdown().await;
    assert!(svc.list(None).is_empty());
    assert!(matches!(svc.open(open_req(dir.path())).await, Err(TerminalError::ShuttingDown)));
}

// ------------------------------------------------- Windows npm shim execution

/// Records what the gate was asked to classify.
#[derive(Default)]
struct RecordingGate(parking_lot::Mutex<Vec<(orkestr_terminal::ShellKind, String)>>);

#[async_trait]
impl CommandGate for RecordingGate {
    async fn check(&self, request: &GateRequest) -> GateDecision {
        self.0.lock().push((request.shell, request.command.clone()));
        GateDecision::Allow
    }
}

#[tokio::test]
async fn command_shell_runs_program_commands_and_gate_sees_inner_command() {
    let dir = tempfile::tempdir().unwrap();
    let gate = Arc::new(RecordingGate::default());
    let (svc, _rx) = service(gate.clone());
    let req = RunRequest {
        root: Some(dir.path().to_path_buf()),
        shell: ShellSpec::Command,
        command: "echo checked".into(),
        stream_to_view: Some(false),
        ..RunRequest::default()
    };
    let out = svc.run(req, CancellationToken::new()).await.unwrap();
    assert_eq!(out.exit_code, Some(0), "{out:?}");
    assert_eq!(out.stdout.trim(), "checked");
    let seen = gate.0.lock().clone();
    assert_eq!(seen.len(), 1);
    // The gate classifies the caller's command verbatim, in the command shell's dialect.
    assert_eq!(seen[0].1, "echo checked");
    if cfg!(windows) {
        assert_eq!(seen[0].0, orkestr_terminal::ShellKind::Cmd);
    }
}

/// `npm` must run on Windows regardless of the PowerShell execution policy
/// (default `Restricted` refuses `npm.ps1`). Skipped when npm is not installed.
#[cfg(windows)]
#[tokio::test]
async fn windows_npm_runs_via_command_shell_and_powershell() {
    if orkestr_platform::process::which("npm.cmd").is_none() {
        eprintln!("npm not installed; skipping");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (svc, _rx) = service(Arc::new(AllowAllGate));
    let is_version = |s: &str| {
        let v = s.trim();
        !v.is_empty() && v.split('.').count() >= 2 && v.chars().all(|c| c.is_ascii_digit() || c == '.')
    };
    let mut shells = vec![ShellSpec::Command, ShellSpec::Id("cmd".into())];
    for id in ["powershell", "pwsh"] {
        if svc.shells(false).iter().any(|s| s.id == id) {
            shells.push(ShellSpec::Id(id.into()));
        }
    }
    for shell in shells {
        for command in ["npm --version", "npx --version"] {
            let req = RunRequest {
                root: Some(dir.path().to_path_buf()),
                shell: shell.clone(),
                command: command.into(),
                timeout: Some(Duration::from_secs(120)),
                stream_to_view: Some(false),
                ..RunRequest::default()
            };
            let out = svc.run(req, CancellationToken::new()).await.unwrap();
            assert_eq!(out.exit_code, Some(0), "{shell:?} `{command}`: {out:?}");
            assert!(is_version(&out.stdout), "{shell:?} `{command}` stdout: {:?} stderr: {:?}", out.stdout, out.stderr);
            assert!(!out.stderr.contains("about_Execution_Policies"), "{shell:?}: {}", out.stderr);
        }
    }
    svc.shutdown().await;
}
