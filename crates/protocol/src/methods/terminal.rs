//! `terminal.*` — terminal sessions and command execution (§15, ADR-0014).
//!
//! Output streams as `TerminalOutput{terminalId, data, offset}` events on
//! `terminal:<id>`; process exit as `TerminalExited`. Clients reconnecting to
//! a session call `terminal.history` and drop live events whose
//! `offset + data.length (bytes)` is ≤ the history's `endOffset`.
//!
//! Agent-owned sessions are read-only for users: `terminal.write` is rejected
//! with `forbidden`; their agent's input goes through `terminal.execLine`,
//! which is gated line by line (`forbidden` when denied, `approval_required`
//! when an approval is needed).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{AgentId, ProjectId, TaskId, TerminalId, Timestamp};

use super::{method, Empty};

/// Shell family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum TerminalShellKind {
    /// PowerShell 7+.
    PowerShell7,
    /// Windows PowerShell 5.1.
    WindowsPowerShell,
    /// cmd.exe.
    Cmd,
    /// Git Bash.
    GitBash,
    /// WSL.
    Wsl,
    /// bash.
    Bash,
    /// zsh.
    Zsh,
    /// fish.
    Fish,
    /// POSIX sh.
    Sh,
    /// Custom program.
    Custom,
}

/// A shell offered for new terminals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ShellDescriptor {
    /// Stable id (`pwsh`, `powershell`, `cmd`, `gitbash`, `wsl`, `bash`, …).
    pub id: String,
    /// Family.
    pub kind: TerminalShellKind,
    /// Display name.
    pub name: String,
    /// Executable path.
    pub path: String,
    /// Interactive arguments.
    pub args: Vec<String>,
    /// Platform default.
    pub is_default: bool,
}

/// Params of `terminal.shells`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListShellsParams {
    /// Re-run shell detection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub refresh: Option<bool>,
}

/// Result of `terminal.shells`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListShellsResult {
    /// Shells, default first.
    pub shells: Vec<ShellDescriptor>,
}

/// Session owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum TerminalOwnerInfo {
    /// The user.
    User,
    /// An agent.
    #[serde(rename_all = "camelCase")]
    Agent {
        /// Agent id.
        agent_id: AgentId,
    },
}

/// Session kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TerminalSessionKind {
    /// Interactive PTY.
    Pty,
    /// Read-only view of agent command output.
    CommandView,
}

/// Session status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TerminalSessionStatus {
    /// Running.
    Running,
    /// Exited.
    Exited,
}

/// A terminal session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TerminalSession {
    /// Id.
    pub terminal_id: TerminalId,
    /// Project.
    pub project_id: Option<ProjectId>,
    /// Owner.
    pub owner: TerminalOwnerInfo,
    /// Kind.
    pub kind: TerminalSessionKind,
    /// Shell id.
    pub shell_id: String,
    /// Shell family.
    pub shell_kind: TerminalShellKind,
    /// Title.
    pub title: String,
    /// Working directory at start.
    pub cwd: String,
    /// Columns.
    pub cols: u16,
    /// Rows.
    pub rows: u16,
    /// Child pid.
    pub pid: Option<u32>,
    /// Status.
    pub status: TerminalSessionStatus,
    /// Exit code (when exited).
    pub exit_code: Option<i32>,
    /// Whether the user may type into it (false for agent terminals and views).
    pub read_only: bool,
    /// Creation time.
    pub created_at: Timestamp,
    /// Total output bytes so far (= history `endOffset`).
    pub output_bytes: u64,
}

/// An explicitly injected environment variable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TerminalEnvVar {
    /// Name (`[A-Za-z_][A-Za-z0-9_]*`).
    pub name: String,
    /// Value.
    pub value: String,
}

/// A custom shell program.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CustomShell {
    /// Program (absolute path or name on PATH).
    pub program: String,
    /// Arguments.
    #[serde(default)]
    pub args: Vec<String>,
}

/// Params of `terminal.open`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct OpenTerminalParams {
    /// Project (cwd is confined to its root). Without a project, `cwd` must be
    /// absolute (default: home directory).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
    /// Shell id from `terminal.shells` (default: platform default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub shell_id: Option<String>,
    /// Custom shell (overrides `shellId`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub custom_shell: Option<CustomShell>,
    /// Working directory (relative to the project root or absolute inside it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub cwd: Option<String>,
    /// Columns (default 120).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub cols: Option<u16>,
    /// Rows (default 30).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub rows: Option<u16>,
    /// Tab title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub title: Option<String>,
    /// Extra environment (secrets only when injected explicitly).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub env: Option<Vec<TerminalEnvVar>>,
    /// Open an agent-owned session for this agent (requires `projectId`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub agent_id: Option<AgentId>,
}

/// Result carrying one session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TerminalSessionResult {
    /// The session.
    pub terminal: TerminalSession,
}

/// Params of `terminal.write`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WriteTerminalParams {
    /// Session.
    pub terminal_id: TerminalId,
    /// Input (keystrokes, pasted text).
    pub data: String,
}

/// Params of `terminal.execLine`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExecLineParams {
    /// Agent-owned session.
    pub terminal_id: TerminalId,
    /// One command line (no line breaks).
    pub line: String,
}

/// Params of `terminal.resize`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ResizeTerminalParams {
    /// Session.
    pub terminal_id: TerminalId,
    /// Columns (1..=1000).
    pub cols: u16,
    /// Rows (1..=500).
    pub rows: u16,
}

/// Params of `terminal.rename`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RenameTerminalParams {
    /// Session.
    pub terminal_id: TerminalId,
    /// New title (1..=80 chars).
    pub title: String,
}

/// Params of `terminal.close`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TerminalIdParams {
    /// Session.
    pub terminal_id: TerminalId,
}

/// Params of `terminal.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListTerminalsParams {
    /// Only sessions of this project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
}

/// Result of `terminal.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListTerminalsResult {
    /// Sessions, oldest first.
    pub terminals: Vec<TerminalSession>,
}

/// Params of `terminal.history`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TerminalHistoryParams {
    /// Session.
    pub terminal_id: TerminalId,
    /// Max bytes from the end (default: whole scrollback).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub max_bytes: Option<u32>,
}

/// Result of `terminal.history`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TerminalHistoryResult {
    /// Session.
    pub terminal_id: TerminalId,
    /// Scrollback text (UTF-8, ANSI included).
    pub data: String,
    /// Absolute byte offset of `data`'s first byte.
    pub start_offset: u64,
    /// Absolute byte offset after the last byte.
    pub end_offset: u64,
    /// Whether older output was dropped.
    pub truncated: bool,
}

/// Params of `terminal.run`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RunCommandParams {
    /// The command line.
    pub command: String,
    /// Project (cwd confined to its root; required for agents).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
    /// Task scope (for history/approvals).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub task_id: Option<TaskId>,
    /// Run on behalf of this agent (gated; streamed to its command view).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub agent_id: Option<AgentId>,
    /// Shell id (dialect); default: platform default shell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub shell_id: Option<String>,
    /// Working directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub cwd: Option<String>,
    /// Extra environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub env: Option<Vec<TerminalEnvVar>>,
    /// Timeout in ms (default 10 min, max 60 min).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub timeout_ms: Option<u64>,
    /// Max captured bytes per stream (default 1 MiB, max 16 MiB).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub max_output_bytes: Option<u32>,
    /// Stream output to a visible command view (default: true for agents).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub stream: Option<bool>,
}

/// Result of `terminal.run`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RunCommandResult {
    /// Exit code (`null` when killed or signalled).
    pub exit_code: Option<i32>,
    /// Captured stdout.
    pub stdout: String,
    /// Captured stderr.
    pub stderr: String,
    /// Output was truncated at `maxOutputBytes`.
    pub truncated: bool,
    /// Killed by the timeout.
    pub timed_out: bool,
    /// Duration in ms.
    pub duration_ms: u64,
    /// Command view the output was streamed to.
    pub terminal_id: Option<TerminalId>,
}

method!(
    /// `terminal.shells` — detected shells.
    Shells, "terminal.shells", ListShellsParams, ListShellsResult
);
method!(
    /// `terminal.open` — opens a PTY session; output streams on `terminal:<id>`.
    Open, "terminal.open", OpenTerminalParams, TerminalSessionResult
);
method!(
    /// `terminal.write` — user keystrokes (user-owned sessions only).
    Write, "terminal.write", WriteTerminalParams, Empty
);
method!(
    /// `terminal.execLine` — one gated line into an agent-owned session.
    ExecLine, "terminal.execLine", ExecLineParams, Empty
);
method!(
    /// `terminal.resize`.
    Resize, "terminal.resize", ResizeTerminalParams, Empty
);
method!(
    /// `terminal.rename`.
    Rename, "terminal.rename", RenameTerminalParams, TerminalSessionResult
);
method!(
    /// `terminal.close` — kills the process tree and removes the session (emits `TerminalExited` if running).
    Close, "terminal.close", TerminalIdParams, Empty
);
method!(
    /// `terminal.list`.
    List, "terminal.list", ListTerminalsParams, ListTerminalsResult
);
method!(
    /// `terminal.history` — scrollback for reconnects.
    History, "terminal.history", TerminalHistoryParams, TerminalHistoryResult
);
method!(
    /// `terminal.run` — runs one command without a PTY (gated, timeout, tree kill).
    Run, "terminal.run", RunCommandParams, RunCommandResult
);

/// Method names of this area (all implemented).
pub const PLANNED: &[&str] = &[
    "terminal.shells",
    "terminal.open",
    "terminal.write",
    "terminal.execLine",
    "terminal.resize",
    "terminal.rename",
    "terminal.close",
    "terminal.list",
    "terminal.history",
    "terminal.run",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planned_names_are_valid() {
        for name in super::PLANNED {
            assert!(crate::methods::is_valid_method_name(name), "{name}");
        }
    }

    #[test]
    fn owner_is_kind_tagged() {
        let a = AgentId::new();
        let v = serde_json::to_value(TerminalOwnerInfo::Agent { agent_id: a }).unwrap();
        assert_eq!(v["kind"], "agent");
        assert_eq!(v["agentId"], a.to_string());
        assert_eq!(serde_json::to_value(TerminalOwnerInfo::User).unwrap()["kind"], "user");
    }

    #[test]
    fn open_params_are_all_optional() {
        let p: OpenTerminalParams = serde_json::from_str("{}").unwrap();
        assert_eq!(p, OpenTerminalParams::default());
    }
}
