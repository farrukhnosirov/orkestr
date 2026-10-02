//! Public data types of the terminal service.

use std::path::PathBuf;
use std::time::Duration;

use orkestr_domain::{AgentId, ProjectId, TaskId, TerminalId, Timestamp};

use crate::env::EnvVar;
use crate::shell::{ShellKind, ShellSpec};

/// Who owns a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TerminalOwner {
    /// The local user (trusted; raw keystrokes).
    User,
    /// An agent (input is line-gated; read-only for users).
    Agent {
        /// The agent.
        agent_id: AgentId,
    },
}

impl TerminalOwner {
    /// Agent id for agent-owned sessions.
    #[must_use]
    pub fn agent_id(self) -> Option<AgentId> {
        match self {
            Self::User => None,
            Self::Agent { agent_id } => Some(agent_id),
        }
    }

    /// Whether this is an agent.
    #[must_use]
    pub fn is_agent(self) -> bool {
        matches!(self, Self::Agent { .. })
    }
}

/// Kind of session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    /// Interactive PTY.
    Pty,
    /// Read-only view streaming the output of agent `run` commands.
    CommandView,
}

/// Lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalStatus {
    /// Process running (or view open).
    Running,
    /// Process exited.
    Exited {
        /// Exit code when known.
        exit_code: Option<i32>,
    },
}

/// Snapshot of a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalInfo {
    /// Session id.
    pub id: TerminalId,
    /// Project scope.
    pub project_id: Option<ProjectId>,
    /// Owner.
    pub owner: TerminalOwner,
    /// Kind.
    pub kind: SessionKind,
    /// Shell id (`pwsh`, `cmd`, `custom`, …).
    pub shell_id: String,
    /// Shell family.
    pub shell_kind: ShellKind,
    /// Display title.
    pub title: String,
    /// Working directory at start.
    pub cwd: PathBuf,
    /// Columns.
    pub cols: u16,
    /// Rows.
    pub rows: u16,
    /// Child pid (PTY sessions).
    pub pid: Option<u32>,
    /// Status.
    pub status: TerminalStatus,
    /// Creation time.
    pub created_at: Timestamp,
    /// Total output bytes so far.
    pub output_bytes: u64,
}

/// Request to open a PTY session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRequest {
    /// Project scope.
    pub project_id: Option<ProjectId>,
    /// Confinement root (project root or worktree). Required for agents.
    pub root: Option<PathBuf>,
    /// Requested cwd (relative to `root`, or absolute inside it).
    pub cwd: Option<PathBuf>,
    /// Owner.
    pub owner: TerminalOwner,
    /// Shell.
    pub shell: ShellSpec,
    /// Explicitly injected environment.
    pub env: Vec<EnvVar>,
    /// Columns.
    pub cols: u16,
    /// Rows.
    pub rows: u16,
    /// Optional title (defaults to the shell name).
    pub title: Option<String>,
}

impl Default for OpenRequest {
    fn default() -> Self {
        Self {
            project_id: None,
            root: None,
            cwd: None,
            owner: TerminalOwner::User,
            shell: ShellSpec::Default,
            env: Vec::new(),
            cols: 120,
            rows: 30,
            title: None,
        }
    }
}

/// Request to run one command without a PTY.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRequest {
    /// Project scope.
    pub project_id: Option<ProjectId>,
    /// Task scope.
    pub task_id: Option<TaskId>,
    /// Confinement root (required for agents).
    pub root: Option<PathBuf>,
    /// cwd (relative to root or absolute inside it).
    pub cwd: Option<PathBuf>,
    /// Owner.
    pub owner: TerminalOwner,
    /// Shell (dialect).
    pub shell: ShellSpec,
    /// The command line.
    pub command: String,
    /// Explicitly injected environment.
    pub env: Vec<EnvVar>,
    /// Timeout (clamped to the service maximum).
    pub timeout: Option<Duration>,
    /// Max captured bytes per stream (clamped to the service maximum).
    pub max_output: Option<usize>,
    /// Stream into a visible command view session (default: on for agents).
    pub stream_to_view: Option<bool>,
}

impl Default for RunRequest {
    fn default() -> Self {
        Self {
            project_id: None,
            task_id: None,
            root: None,
            cwd: None,
            owner: TerminalOwner::User,
            shell: ShellSpec::Default,
            command: String::new(),
            env: Vec::new(),
            timeout: None,
            max_output: None,
            stream_to_view: None,
        }
    }
}

/// Result of [`RunRequest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutput {
    /// Exit code (`None` when killed by timeout/cancel or a signal).
    pub exit_code: Option<i32>,
    /// Captured stdout (lossy UTF-8, possibly truncated).
    pub stdout: String,
    /// Captured stderr (lossy UTF-8, possibly truncated).
    pub stderr: String,
    /// Whether any stream exceeded `max_output`.
    pub truncated: bool,
    /// Whether the command was killed by the timeout.
    pub timed_out: bool,
    /// Wall-clock duration.
    pub duration: Duration,
    /// The view session the output was streamed to.
    pub view_terminal_id: Option<TerminalId>,
}

/// Limits and defaults of the service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalConfig {
    /// Max concurrently registered sessions (PTY + views).
    pub max_sessions: usize,
    /// Scrollback bytes per session.
    pub scrollback_bytes: usize,
    /// Output coalescing window.
    pub coalesce_window: Duration,
    /// Max bytes per output event.
    pub max_event_bytes: usize,
    /// Default `run` timeout.
    pub default_run_timeout: Duration,
    /// Max `run` timeout.
    pub max_run_timeout: Duration,
    /// Default captured bytes per stream.
    pub default_max_output: usize,
    /// Max captured bytes per stream.
    pub max_output_cap: usize,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            max_sessions: 32,
            scrollback_bytes: crate::scrollback::DEFAULT_SCROLLBACK_BYTES,
            coalesce_window: Duration::from_millis(16),
            max_event_bytes: 32 * 1024,
            default_run_timeout: Duration::from_secs(10 * 60),
            max_run_timeout: Duration::from_secs(60 * 60),
            default_max_output: 1024 * 1024,
            max_output_cap: 16 * 1024 * 1024,
        }
    }
}
