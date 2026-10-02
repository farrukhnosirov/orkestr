//! A registered session and its output pump.
//!
//! Output path: PTY reader thread → bounded channel → pump task (UTF-8
//! decoding, coalescing into ≤ `coalesce_window` / ≤ `max_event_bytes`
//! batches) → scrollback + [`TerminalSink`]. Exit path: waiter thread →
//! `Exited` → the pump drains the remaining output (on Windows ConPTY only
//! reaches EOF once the pseudo console is closed, so the PTY is released
//! after a short grace period) → `TerminalExited`.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use orkestr_domain::{ProjectId, TerminalId, Timestamp};
use parking_lot::Mutex;
use portable_pty::{ChildKiller, MasterPty, PtySize};
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

use crate::error::{TerminalError, TerminalResult};
use crate::gate::LineAssembler;
use crate::scrollback::{HistorySlice, Scrollback};
use crate::shell::ShellKind;
use crate::sink::{SinkMeta, TerminalSink};
use crate::types::{SessionKind, TerminalInfo, TerminalOwner, TerminalStatus};
use crate::utf8::{split_utf8, Utf8Decoder};

/// Messages into a session's pump.
#[derive(Debug)]
pub(crate) enum PumpMsg {
    /// Raw PTY bytes.
    Data(Vec<u8>),
    /// Already-decoded text (command views, notices).
    Text(String),
    /// Reader reached end of stream.
    Eof,
    /// The process exited (or a view was closed).
    Exited(Option<i32>),
}

/// Capacity of the pump channel (messages of ≤ 8 KiB).
pub(crate) const PUMP_CAPACITY: usize = 256;

/// Grace period for trailing output after exit before the PTY is released.
const EXIT_DRAIN: Duration = Duration::from_millis(250);
/// Wait for EOF after the PTY was released.
const RELEASE_DRAIN: Duration = Duration::from_secs(2);
/// Wait for the exit status after EOF.
const EOF_EXIT_WAIT: Duration = Duration::from_secs(5);

/// PTY handles kept alive while the session runs.
pub(crate) struct PtyHandles {
    pub(crate) master: Box<dyn MasterPty + Send>,
    pub(crate) killer: Box<dyn ChildKiller + Send + Sync>,
}

impl Drop for PtyHandles {
    fn drop(&mut self) {
        // Best effort; closing the master also closes the pseudo console.
        let _ = self.killer.kill();
    }
}

/// Mutable session state.
#[derive(Debug, Clone)]
pub(crate) struct SessionState {
    pub(crate) title: String,
    pub(crate) cols: u16,
    pub(crate) rows: u16,
    pub(crate) status: TerminalStatus,
}

/// A registered session.
pub(crate) struct Session {
    pub(crate) id: TerminalId,
    pub(crate) project_id: Option<ProjectId>,
    pub(crate) owner: TerminalOwner,
    pub(crate) kind: SessionKind,
    pub(crate) shell_id: String,
    pub(crate) shell_kind: ShellKind,
    pub(crate) cwd: PathBuf,
    pub(crate) created_at: Timestamp,
    pub(crate) pid: Option<u32>,
    pub(crate) state: Mutex<SessionState>,
    pub(crate) scrollback: Mutex<Scrollback>,
    pub(crate) pump_tx: mpsc::Sender<PumpMsg>,
    pub(crate) pty: Mutex<Option<PtyHandles>>,
    pub(crate) writer: Arc<Mutex<Option<Box<dyn Write + Send>>>>,
    /// Serializes agent input (held across gate checks).
    pub(crate) input: tokio::sync::Mutex<LineAssembler>,
    pub(crate) done: watch::Sender<bool>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session").field("id", &self.id).field("kind", &self.kind).finish_non_exhaustive()
    }
}

impl Session {
    pub(crate) fn meta(&self) -> SinkMeta {
        SinkMeta { terminal_id: self.id, project_id: self.project_id, agent_id: self.owner.agent_id() }
    }

    pub(crate) fn info(&self) -> TerminalInfo {
        let st = self.state.lock().clone();
        TerminalInfo {
            id: self.id,
            project_id: self.project_id,
            owner: self.owner,
            kind: self.kind,
            shell_id: self.shell_id.clone(),
            shell_kind: self.shell_kind,
            title: st.title,
            cwd: self.cwd.clone(),
            cols: st.cols,
            rows: st.rows,
            pid: self.pid,
            status: st.status,
            created_at: self.created_at,
            output_bytes: self.scrollback.lock().end_offset(),
        }
    }

    pub(crate) fn is_running(&self) -> bool {
        matches!(self.state.lock().status, TerminalStatus::Running)
    }

    pub(crate) fn history(&self, max_bytes: Option<usize>) -> HistorySlice {
        self.scrollback.lock().tail(max_bytes)
    }

    /// Queues a notice line (dim/yellow) into the output stream.
    pub(crate) fn notice(&self, text: &str) {
        let line = format!("\r\n\x1b[33m[orkestr] {text}\x1b[0m\r\n");
        if self.pump_tx.try_send(PumpMsg::Text(line)).is_err() {
            tracing::debug!(terminal_id = %self.id, "notice dropped (pump busy or finished)");
        }
    }

    /// Writes raw bytes to the PTY (blocking pool).
    pub(crate) async fn write_bytes(&self, data: Vec<u8>) -> TerminalResult<()> {
        let writer = Arc::clone(&self.writer);
        let id = self.id;
        tokio::task::spawn_blocking(move || {
            let mut guard = writer.lock();
            let w = guard.as_mut().ok_or(TerminalError::Exited(id))?;
            w.write_all(&data)?;
            w.flush()?;
            Ok(())
        })
        .await
        .map_err(|e| TerminalError::Pty(format!("writer task failed: {e}")))?
    }

    pub(crate) fn resize_pty(&self, cols: u16, rows: u16) -> TerminalResult<()> {
        if let Some(pty) = self.pty.lock().as_ref() {
            pty.master
                .resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
                .map_err(|e| TerminalError::Pty(e.to_string()))?;
        }
        Ok(())
    }

    /// Drops the PTY (closes the pseudo console) and the writer.
    pub(crate) async fn release_pty(&self) {
        let handles = self.pty.lock().take();
        let writer = self.writer.lock().take();
        if handles.is_some() || writer.is_some() {
            // ClosePseudoConsole may block briefly; keep it off the async workers.
            let _ = tokio::task::spawn_blocking(move || {
                drop(writer);
                drop(handles);
            })
            .await;
        }
    }

    /// Kills the child via the PTY killer (shell only; the tree is killed by the caller).
    pub(crate) fn kill_child(&self) {
        if let Some(pty) = self.pty.lock().as_mut() {
            if let Err(e) = pty.killer.kill() {
                tracing::debug!(terminal_id = %self.id, error = %e, "child kill failed (probably already exited)");
            }
        }
    }

    /// Waits until the pump finished (`TerminalExited` sent).
    pub(crate) async fn wait_done(&self, timeout: Duration) -> bool {
        let mut rx = self.done.subscribe();
        tokio::time::timeout(timeout, rx.wait_for(|d| *d)).await.map(|r| r.is_ok()).unwrap_or(false)
    }
}

/// Pump settings.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PumpConfig {
    pub(crate) window: Duration,
    pub(crate) max_event_bytes: usize,
}

/// Runs the output pump until the session has exited and its output drained.
pub(crate) async fn pump(
    session: Arc<Session>,
    mut rx: mpsc::Receiver<PumpMsg>,
    sink: Arc<dyn TerminalSink>,
    cfg: PumpConfig,
    has_reader: bool,
) {
    let meta = session.meta();
    let mut decoder = Utf8Decoder::new();
    let mut pending = String::new();
    let mut flush_at: Option<Instant> = None;
    let mut exit: Option<Option<i32>> = None;
    let mut eof = !has_reader;
    let mut drain_at: Option<Instant> = None;
    let mut released = false;
    let mut handshake = has_reader.then(CursorHandshake::default);

    loop {
        if exit.is_some() && eof {
            break;
        }
        let timer = match (flush_at, drain_at) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        tokio::select! {
            msg = rx.recv() => match msg {
                Some(PumpMsg::Data(bytes)) => {
                    let mut text = decoder.decode(&bytes);
                    if let Some(h) = handshake.as_mut() {
                        match h.filter(&mut text) {
                            HandshakeStep::Pending => {}
                            HandshakeStep::Answer => {
                                handshake = None;
                                if let Err(e) = session.write_bytes(CURSOR_REPORT.to_vec()).await {
                                    tracing::debug!(terminal_id = %session.id, error = %e, "cursor report failed");
                                }
                            }
                            HandshakeStep::Done => handshake = None,
                        }
                    }
                    pending.push_str(&text);
                }
                Some(PumpMsg::Text(text)) => pending.push_str(&text),
                Some(PumpMsg::Eof) => {
                    eof = true;
                    pending.push_str(&decoder.finish());
                    if exit.is_none() {
                        drain_at = Some(Instant::now() + EOF_EXIT_WAIT);
                    }
                }
                Some(PumpMsg::Exited(code)) => {
                    if exit.is_none() {
                        exit = Some(code);
                        if !eof {
                            drain_at = Some(Instant::now() + EXIT_DRAIN);
                        }
                    }
                }
                None => {
                    eof = true;
                    exit.get_or_insert(None);
                }
            },
            () = sleep_until_opt(timer), if timer.is_some() => {
                let now = Instant::now();
                if flush_at.is_some_and(|t| t <= now) {
                    flush(&session, &sink, meta, &mut pending, cfg.max_event_bytes).await;
                    flush_at = None;
                }
                if drain_at.is_some_and(|t| t <= now) {
                    drain_at = None;
                    match (exit.is_some(), eof, released) {
                        (true, false, false) => {
                            // Exited but no EOF yet (ConPTY): close the console to unblock the reader.
                            session.release_pty().await;
                            released = true;
                            drain_at = Some(Instant::now() + RELEASE_DRAIN);
                        }
                        (true, false, true) => {
                            tracing::warn!(terminal_id = %session.id, "PTY reader did not finish after release; giving up");
                            eof = true;
                        }
                        (false, true, _) => {
                            tracing::debug!(terminal_id = %session.id, "EOF without exit status");
                            exit = Some(None);
                        }
                        _ => {}
                    }
                }
            }
        }
        if pending.len() >= cfg.max_event_bytes {
            flush(&session, &sink, meta, &mut pending, cfg.max_event_bytes).await;
            flush_at = None;
        } else if !pending.is_empty() && flush_at.is_none() {
            flush_at = Some(Instant::now() + cfg.window);
        }
    }
    pending.push_str(&decoder.finish());
    flush(&session, &sink, meta, &mut pending, cfg.max_event_bytes).await;
    session.release_pty().await;
    let code = exit.flatten();
    session.state.lock().status = TerminalStatus::Exited { exit_code: code };
    tracing::info!(terminal_id = %session.id, exit_code = ?code, "terminal exited");
    sink.exited(meta, code).await;
    let _ = session.done.send(true);
}

/// Cursor position request (DSR 6).
const CURSOR_REQUEST: &str = "\x1b[6n";
/// Our answer: cursor at row 1, column 1.
const CURSOR_REPORT: &[u8] = b"\x1b[1;1R";
/// Only the start of a session is inspected for the handshake.
const HANDSHAKE_WINDOW: usize = 16 * 1024;

/// ConPTY (and some shells) ask for the cursor position right after start and
/// block until it is answered. The core answers it itself — clients may not
/// be attached yet (agent sessions, headless) — and removes the request from
/// the stream so that history replays in xterm.js don't answer it again
/// (which would type a stray `ESC[1;1R` into the shell).
#[derive(Debug, Default)]
struct CursorHandshake {
    seen: usize,
}

enum HandshakeStep {
    Pending,
    Answer,
    Done,
}

impl CursorHandshake {
    fn filter(&mut self, text: &mut String) -> HandshakeStep {
        if let Some(pos) = text.find(CURSOR_REQUEST) {
            text.replace_range(pos..pos + CURSOR_REQUEST.len(), "");
            return HandshakeStep::Answer;
        }
        self.seen += text.len();
        if self.seen > HANDSHAKE_WINDOW {
            HandshakeStep::Done
        } else {
            HandshakeStep::Pending
        }
    }
}

async fn sleep_until_opt(t: Option<Instant>) {
    match t {
        Some(t) => tokio::time::sleep_until(t).await,
        None => std::future::pending().await,
    }
}

async fn flush(session: &Session, sink: &Arc<dyn TerminalSink>, meta: SinkMeta, pending: &mut String, max: usize) {
    if pending.is_empty() {
        return;
    }
    let text = std::mem::take(pending);
    for piece in split_utf8(&text, max) {
        let offset = session.scrollback.lock().push(piece);
        sink.output(meta, offset, piece.to_owned()).await;
    }
}

/// Spawns the blocking PTY reader thread.
pub(crate) fn spawn_reader(
    id: TerminalId,
    mut reader: Box<dyn std::io::Read + Send>,
    tx: mpsc::Sender<PumpMsg>,
) -> std::io::Result<()> {
    std::thread::Builder::new().name(format!("pty-read-{id}")).spawn(move || {
        let mut buf = vec![0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx.blocking_send(PumpMsg::Data(buf[..n].to_vec())).is_err() {
                        return;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    // EIO on Unix when the child side closes; normal end of stream.
                    tracing::debug!(terminal_id = %id, error = %e, "pty read ended");
                    break;
                }
            }
        }
        let _ = tx.blocking_send(PumpMsg::Eof);
    })?;
    Ok(())
}

/// Spawns the blocking waiter thread reporting the exit status.
pub(crate) fn spawn_waiter(
    id: TerminalId,
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
    tx: mpsc::Sender<PumpMsg>,
) -> std::io::Result<()> {
    std::thread::Builder::new().name(format!("pty-wait-{id}")).spawn(move || {
        let code = match child.wait() {
            Ok(status) => {
                if status.signal().is_some() {
                    None
                } else {
                    Some(i32::from_ne_bytes(status.exit_code().to_ne_bytes()))
                }
            }
            Err(e) => {
                tracing::warn!(terminal_id = %id, error = %e, "waiting for pty child failed");
                None
            }
        };
        let _ = tx.blocking_send(PumpMsg::Exited(code));
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_request_is_answered_once_and_stripped() {
        let mut h = CursorHandshake::default();
        let mut t = String::from("\x1b[?9001h\x1b[6nhello");
        assert!(matches!(h.filter(&mut t), HandshakeStep::Answer));
        assert_eq!(t, "\x1b[?9001hhello");
        let mut h = CursorHandshake::default();
        let mut plain = "x".repeat(HANDSHAKE_WINDOW + 1);
        assert!(matches!(h.filter(&mut plain), HandshakeStep::Done));
        let mut h = CursorHandshake::default();
        let mut small = String::from("prompt> ");
        assert!(matches!(h.filter(&mut small), HandshakeStep::Pending));
    }
}
