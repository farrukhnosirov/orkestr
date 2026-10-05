//! Command execution seam. Production uses [`TerminalExecutor`] — the
//! terminal's run API, which passes every command through the security
//! `CommandGate` (permissions, dangerous-action guard, approvals), confines
//! the cwd to the workspace root, enforces timeouts, kills the process tree
//! on cancellation and streams output into a visible command view.

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use orkestr_domain::{AgentId, ProjectId, TaskId};
use orkestr_terminal::{EnvVar, RunRequest, ShellSpec, TerminalError, TerminalOwner, TerminalService};
use tokio_util::sync::CancellationToken;

/// One command to execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecRequest {
    /// Project scope.
    pub project_id: ProjectId,
    /// Task scope.
    pub task_id: Option<TaskId>,
    /// Agent on whose behalf the command runs (`None` = the user).
    pub agent_id: Option<AgentId>,
    /// Confinement root (project root or worktree).
    pub root: PathBuf,
    /// cwd relative to `root`.
    pub cwd: PathBuf,
    /// Command line.
    pub command: String,
    /// Timeout.
    pub timeout: Duration,
    /// Max captured bytes per stream.
    pub max_output: usize,
}

/// Captured result.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecOutput {
    /// Exit code (`None` when killed).
    pub exit_code: Option<i32>,
    /// stdout (possibly truncated).
    pub stdout: String,
    /// stderr (possibly truncated).
    pub stderr: String,
    /// Killed by the timeout.
    pub timed_out: bool,
    /// Wall-clock duration.
    pub duration: Duration,
}

/// Execution failures that did not produce output.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExecError {
    /// The security gate denied the command.
    #[error("denied: {0}")]
    Denied(String),
    /// The command awaits a user approval.
    #[error("approval required: {reason}")]
    ApprovalRequired {
        /// Why.
        reason: String,
        /// Approval request id.
        approval_id: Option<String>,
    },
    /// Cancelled before/while running.
    #[error("cancelled")]
    Cancelled,
    /// Spawn / I/O / validation failure.
    #[error("{0}")]
    Failed(String),
}

/// Runs check commands.
#[async_trait]
pub trait CheckExecutor: Send + Sync {
    /// Executes one command.
    async fn execute(&self, req: ExecRequest, cancel: CancellationToken) -> Result<ExecOutput, ExecError>;
}

/// [`CheckExecutor`] over the terminal service.
#[derive(Clone)]
pub struct TerminalExecutor {
    terminals: TerminalService,
}

impl std::fmt::Debug for TerminalExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalExecutor").finish_non_exhaustive()
    }
}

impl TerminalExecutor {
    /// Wraps the terminal service.
    #[must_use]
    pub fn new(terminals: TerminalService) -> Self {
        Self { terminals }
    }
}

#[async_trait]
impl CheckExecutor for TerminalExecutor {
    async fn execute(&self, req: ExecRequest, cancel: CancellationToken) -> Result<ExecOutput, ExecError> {
        let owner = match req.agent_id {
            Some(agent_id) => TerminalOwner::Agent { agent_id },
            None => TerminalOwner::User,
        };
        let run = RunRequest {
            project_id: Some(req.project_id),
            task_id: req.task_id,
            root: Some(req.root),
            cwd: Some(req.cwd),
            owner,
            // Detected checks are program invocations (`npm run test`,
            // `cargo test`, …): run them in the platform's command shell —
            // `cmd.exe` on Windows, so `npm` resolves to `npm.cmd` instead of
            // the execution-policy-blocked `npm.ps1`; the default shell on
            // macOS/Linux. The gate classifies `command` in that dialect.
            shell: ShellSpec::Command,
            command: req.command,
            // Non-interactive, colorless output: runners stay in single-run
            // mode and parsers see plain text.
            env: vec![
                EnvVar::new("CI", "1"),
                EnvVar::new("NO_COLOR", "1"),
                EnvVar::new("FORCE_COLOR", "0"),
                EnvVar::new("CARGO_TERM_COLOR", "never"),
            ],
            timeout: Some(req.timeout),
            max_output: Some(req.max_output),
            stream_to_view: Some(true),
        };
        match self.terminals.run(run, cancel).await {
            Ok(out) => Ok(ExecOutput {
                exit_code: out.exit_code,
                stdout: out.stdout,
                stderr: out.stderr,
                timed_out: out.timed_out,
                duration: out.duration,
            }),
            Err(TerminalError::Denied(r)) => Err(ExecError::Denied(r)),
            Err(TerminalError::ApprovalRequired { reason, approval_id }) => {
                Err(ExecError::ApprovalRequired { reason, approval_id })
            }
            Err(TerminalError::Cancelled) => Err(ExecError::Cancelled),
            Err(e) => Err(ExecError::Failed(e.to_string())),
        }
    }
}
