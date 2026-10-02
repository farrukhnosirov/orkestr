//! Output sink: where coalesced terminal output and exits go.
//!
//! The core implements it by publishing `TerminalOutput` / `TerminalExited`
//! events on `terminal:<id>`. Calls for one session are made sequentially
//! from that session's pump task, so ordering is preserved.

use async_trait::async_trait;
use orkestr_domain::{AgentId, ProjectId, TerminalId};

/// Session identity passed to the sink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SinkMeta {
    /// Session.
    pub terminal_id: TerminalId,
    /// Project scope.
    pub project_id: Option<ProjectId>,
    /// Owning agent.
    pub agent_id: Option<AgentId>,
}

/// Receives terminal output.
#[async_trait]
pub trait TerminalSink: Send + Sync + 'static {
    /// A chunk of output (valid UTF-8, never splits a code point) starting at
    /// absolute byte `offset` of the session stream.
    async fn output(&self, meta: SinkMeta, offset: u64, data: String);
    /// The session's process exited (sent once, after the last output).
    async fn exited(&self, meta: SinkMeta, exit_code: Option<i32>);
}

/// Discards everything.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullSink;

#[async_trait]
impl TerminalSink for NullSink {
    async fn output(&self, _meta: SinkMeta, _offset: u64, _data: String) {}
    async fn exited(&self, _meta: SinkMeta, _exit_code: Option<i32>) {}
}

/// What a [`ChannelSink`] forwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SinkEvent {
    /// Output chunk.
    Output {
        /// Session.
        terminal_id: TerminalId,
        /// Offset.
        offset: u64,
        /// Data.
        data: String,
    },
    /// Exit.
    Exited {
        /// Session.
        terminal_id: TerminalId,
        /// Exit code.
        exit_code: Option<i32>,
    },
}

/// Forwards sink calls into an unbounded channel (tests, embedding).
#[derive(Debug, Clone)]
pub struct ChannelSink {
    tx: tokio::sync::mpsc::UnboundedSender<SinkEvent>,
}

impl ChannelSink {
    /// New sink and its receiver.
    #[must_use]
    pub fn new() -> (Self, tokio::sync::mpsc::UnboundedReceiver<SinkEvent>) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        (Self { tx }, rx)
    }
}

#[async_trait]
impl TerminalSink for ChannelSink {
    async fn output(&self, meta: SinkMeta, offset: u64, data: String) {
        let _ = self.tx.send(SinkEvent::Output { terminal_id: meta.terminal_id, offset, data });
    }
    async fn exited(&self, meta: SinkMeta, exit_code: Option<i32>) {
        let _ = self.tx.send(SinkEvent::Exited { terminal_id: meta.terminal_id, exit_code });
    }
}
