//! # orkestr-terminal
//!
//! Terminal sessions and command execution (MASTER_SPEC §15, ADR-0014):
//!
//! * **Interactive PTY sessions** via `portable-pty` (ConPTY on Windows) with
//!   shells from `orkestr-platform`'s detector, cwd confined to the project
//!   root/worktree, a sanitized environment, resize, process-tree kill and
//!   exit status.
//! * **Output pump**: reader thread → UTF-8-safe coalescing (≈16 ms / 32 KiB)
//!   → [`TerminalSink`] (the core publishes `TerminalOutput` on
//!   `terminal:<id>`) + a 2 MiB scrollback ring for `terminal.history`.
//! * **Agent safety**: agent-owned PTYs are read-only for users and accept
//!   input from their agent only line-by-line through the [`CommandGate`];
//!   agent `run` commands are gated *before* anything is spawned.
//! * **Command execution** without a PTY ([`TerminalService::run`]) with
//!   timeout/cancellation killing the whole process tree, capped output and
//!   streaming into a visible, read-only command view session.
//! * **History** of gated commands (`terminal_commands`, redacted).
//!
//! The terminal is never exposed as a raw port (§64); everything goes through
//! the core's RPC.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod confine;
pub mod env;
pub mod error;
pub mod gate;
pub mod history;
pub mod kill;
pub mod scrollback;
mod service;
mod session;
pub mod shell;
pub mod sink;
pub mod types;
pub mod utf8;

pub use env::EnvVar;
pub use error::{TerminalError, TerminalResult};
pub use gate::{AllowAllGate, CommandGate, GateDecision, GateMode, GateRequest, UserOnlyGate};
pub use history::{terminal_migrations, CommandLog, CommandRecord, NoopCommandLog, SqliteCommandLog};
pub use scrollback::HistorySlice;
pub use service::{TerminalService, TerminalServiceBuilder, MAX_COLS, MAX_ROWS, MAX_TITLE};
pub use shell::{ShellKind, ShellOption, ShellSpec};
pub use sink::{ChannelSink, NullSink, SinkEvent, SinkMeta, TerminalSink};
pub use types::{
    OpenRequest, RunOutput, RunRequest, SessionKind, TerminalConfig, TerminalInfo, TerminalOwner, TerminalStatus,
};
