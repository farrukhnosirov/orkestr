//! Command gate: the security check that runs **before** an agent command
//! reaches a shell (§23, ADR-0010, ADR-0014).
//!
//! The terminal crate does not know the permission engine; it asks a
//! [`CommandGate`] that the core wires to the security area's
//! `PermissionEngine` + `DangerousActionGuard` + approvals. Until that exists
//! the core uses a conservative gate (see `orkestr-core`).

use std::path::PathBuf;

use async_trait::async_trait;
use orkestr_domain::{AgentId, ProjectId, TaskId, TerminalId};

use crate::shell::ShellKind;
use crate::types::TerminalOwner;

/// How the command is going to be executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateMode {
    /// Non-interactive `run` (captured output).
    Run,
    /// A line typed into an agent-owned interactive PTY.
    InteractiveLine,
}

/// What the gate is asked to decide on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateRequest {
    /// Who runs it.
    pub owner: TerminalOwner,
    /// Project scope.
    pub project_id: Option<ProjectId>,
    /// Task scope (when known).
    pub task_id: Option<TaskId>,
    /// Shell family (decides the dialect used for classification).
    pub shell: ShellKind,
    /// The command line, exactly as it will be executed.
    pub command: String,
    /// Working directory.
    pub cwd: PathBuf,
    /// Execution mode.
    pub mode: GateMode,
    /// Session the command belongs to (PTY or command view).
    pub terminal_id: Option<TerminalId>,
}

impl GateRequest {
    /// Agent issuing the command, if any.
    #[must_use]
    pub fn agent_id(&self) -> Option<AgentId> {
        self.owner.agent_id()
    }
}

/// Gate verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateDecision {
    /// Run it.
    Allow,
    /// Never run it.
    Deny {
        /// Human-readable reason (safe to show and log).
        reason: String,
    },
    /// Needs a user approval first; the caller retries after approval.
    Ask {
        /// Why approval is needed.
        reason: String,
        /// Approval request id, when the gate created one.
        approval_id: Option<String>,
    },
}

impl GateDecision {
    /// Short label for logs/history (`allow`, `deny`, `ask`).
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny { .. } => "deny",
            Self::Ask { .. } => "ask",
        }
    }
}

/// Decides whether a command may run.
#[async_trait]
pub trait CommandGate: Send + Sync + 'static {
    /// Checks the command. Must not execute anything.
    async fn check(&self, request: &GateRequest) -> GateDecision;
}

/// Allows everything. **Only** for user-owned commands and tests.
#[derive(Debug, Default, Clone, Copy)]
pub struct AllowAllGate;

#[async_trait]
impl CommandGate for AllowAllGate {
    async fn check(&self, _request: &GateRequest) -> GateDecision {
        GateDecision::Allow
    }
}

/// Allows user-owned commands and requires approval for every agent command.
/// The safe default until the security engine is wired in.
#[derive(Debug, Default, Clone, Copy)]
pub struct UserOnlyGate;

#[async_trait]
impl CommandGate for UserOnlyGate {
    async fn check(&self, request: &GateRequest) -> GateDecision {
        match request.owner {
            TerminalOwner::User => GateDecision::Allow,
            TerminalOwner::Agent { .. } => GateDecision::Ask {
                reason: "agent commands need approval (permission engine not configured)".into(),
                approval_id: None,
            },
        }
    }
}

/// Splits agent keystrokes into complete lines for gating.
///
/// Bytes are buffered until `\r` or `\n`; each complete, non-empty line is
/// returned. `\x03` (Ctrl+C) and `\x04` (Ctrl+D) are passed through as
/// control actions without gating (they can only stop things).
#[derive(Debug, Default, Clone)]
pub struct LineAssembler {
    buf: String,
}

/// One unit of agent input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentInput {
    /// A complete command line (without terminator).
    Line(String),
    /// A control byte (Ctrl+C / Ctrl+D) sent through unchanged.
    Control(u8),
}

/// Max buffered partial line (bytes).
pub const MAX_PENDING_LINE: usize = 64 * 1024;

impl LineAssembler {
    /// Feeds text; returns the complete inputs.
    pub fn feed(&mut self, text: &str) -> Result<Vec<AgentInput>, crate::TerminalError> {
        let mut out = Vec::new();
        for ch in text.chars() {
            match ch {
                '\r' | '\n' => {
                    let line = std::mem::take(&mut self.buf);
                    if !line.trim().is_empty() {
                        out.push(AgentInput::Line(line));
                    }
                }
                '\u{3}' | '\u{4}' => {
                    self.buf.clear();
                    out.push(AgentInput::Control(ch as u8));
                }
                c if c.is_control() && c != '\t' => {
                    // Escape sequences / other control bytes are not allowed from agents.
                    return Err(crate::TerminalError::Forbidden(format!(
                        "control character U+{:04X} is not allowed in agent input",
                        c as u32
                    )));
                }
                c => {
                    if self.buf.len() >= MAX_PENDING_LINE {
                        return Err(crate::TerminalError::Invalid("agent input line too long".into()));
                    }
                    self.buf.push(c);
                }
            }
        }
        Ok(out)
    }

    /// Pending partial line.
    #[must_use]
    pub fn pending(&self) -> &str {
        &self.buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembles_lines_across_feeds() {
        let mut a = LineAssembler::default();
        assert_eq!(a.feed("git st").unwrap(), vec![]);
        assert_eq!(a.pending(), "git st");
        assert_eq!(
            a.feed("atus\r\nls\n").unwrap(),
            vec![AgentInput::Line("git status".into()), AgentInput::Line("ls".into())]
        );
        assert_eq!(a.pending(), "");
    }

    #[test]
    fn control_bytes_pass_and_escapes_are_rejected() {
        let mut a = LineAssembler::default();
        assert_eq!(a.feed("half\u{3}").unwrap(), vec![AgentInput::Control(3)]);
        assert_eq!(a.pending(), "");
        assert!(a.feed("\u{1b}[A").is_err());
    }

    #[test]
    fn blank_lines_are_ignored() {
        let mut a = LineAssembler::default();
        assert!(a.feed("\r\n   \n").unwrap().is_empty());
    }

    #[tokio::test]
    async fn user_only_gate() {
        let mut req = GateRequest {
            owner: TerminalOwner::User,
            project_id: None,
            task_id: None,
            shell: ShellKind::Bash,
            command: "ls".into(),
            cwd: PathBuf::from("/"),
            mode: GateMode::Run,
            terminal_id: None,
        };
        assert_eq!(UserOnlyGate.check(&req).await, GateDecision::Allow);
        req.owner = TerminalOwner::Agent { agent_id: AgentId::new() };
        assert_eq!(UserOnlyGate.check(&req).await.label(), "ask");
        assert_eq!(AllowAllGate.check(&req).await, GateDecision::Allow);
    }
}
