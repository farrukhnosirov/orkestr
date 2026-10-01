//! # orkestr-terminal — **SKELETON** (Phase 1 fills it in)
//!
//! Terminal sessions (MASTER_SPEC §15): PTYs via `portable-pty` (ConPTY on
//! Windows), shells from `orkestr-platform::ShellDetector`, user- and
//! agent-owned sessions, output streamed as `TerminalOutput` events on
//! `terminal:<id>`, history, and **approval interception**: agent commands are
//! classified by the dangerous-action guard before they reach the PTY (§62.11).
//! The terminal is never exposed as a raw port (§64).
#![forbid(unsafe_code)]
#![warn(missing_docs)]

use async_trait::async_trait;
use orkestr_domain::{AgentId, ProjectId, TerminalId};

/// Errors.
#[derive(Debug, thiserror::Error)]
pub enum TerminalError {
    /// Unknown session.
    #[error("terminal {0} not found")]
    NotFound(TerminalId),
    /// PTY failure.
    #[error("pty: {0}")]
    Pty(String),
    /// Command blocked pending approval.
    #[error("command requires approval: {0}")]
    ApprovalRequired(String),
    /// Explicit stub marker.
    #[error("unimplemented-feature: {0}")]
    Unimplemented(&'static str),
}

/// How to open a terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSpec {
    /// Project (cwd defaults to its root).
    pub project_id: Option<ProjectId>,
    /// Owner agent (None = user terminal).
    pub agent_id: Option<AgentId>,
    /// Shell executable (None = platform default).
    pub shell: Option<std::path::PathBuf>,
    /// Working directory.
    pub cwd: Option<std::path::PathBuf>,
    /// Columns.
    pub cols: u16,
    /// Rows.
    pub rows: u16,
}

/// Terminal session manager.
#[async_trait]
pub trait TerminalManager: Send + Sync {
    /// Opens a session; output streams as events.
    async fn open(&self, spec: TerminalSpec) -> Result<TerminalId, TerminalError>;
    /// Writes input (user keystrokes or an approved agent command).
    async fn write(&self, id: TerminalId, data: &[u8]) -> Result<(), TerminalError>;
    /// Resizes.
    async fn resize(&self, id: TerminalId, cols: u16, rows: u16) -> Result<(), TerminalError>;
    /// Closes and kills the process tree.
    async fn close(&self, id: TerminalId) -> Result<(), TerminalError>;
}
