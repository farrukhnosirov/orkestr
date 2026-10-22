//! Log sources (files, deployment logs, ingest). The [`LogSource`] trait is
//! binding (WP6 collectors use it). **Owner: WP5.**
//!
//! Every source is a [`BufferedSource`]: a bounded in-memory ring of
//! **already redacted** lines plus a broadcast channel for live tail. Feeding
//! is push-based (file tailer task, deployment poller, `monitor.ingest`), so
//! all three kinds share the query / tail code. `broadcast` never blocks the
//! sender: a slow consumer lags and its [`TailReceiver`] counts the dropped
//! lines.

use std::collections::{BTreeMap, VecDeque};
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use orkestr_domain::{DeploymentId, ProjectId, Timestamp};
use orkestr_protocol::event::LogLevel;
use orkestr_protocol::methods::deploy::DeployLogsResult;
use orkestr_protocol::methods::monitor::{LogLine, LogQuery, LogSourceInfo};
use parking_lot::Mutex;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::MonitorError;

/// A source of (redacted) log lines.
#[async_trait]
pub trait LogSource: Send + Sync {
    /// Source id.
    fn id(&self) -> &str;
    /// Queries stored / tailed lines.
    async fn query(&self, q: &LogQuery) -> Result<Vec<LogLine>, MonitorError>;
    /// Live tail (lagging receivers drop lines: backpressure never blocks the source).
    fn tail(&self) -> broadcast::Receiver<LogLine>;
}

/// Default lines kept per source.
pub const DEFAULT_RING: usize = 10_000;
/// Default broadcast buffer per source.
pub const DEFAULT_TAIL_BUFFER: usize = 1024;
/// Longest stored line (bytes); longer lines are cut.
pub const MAX_LINE_BYTES: usize = 8 * 1024;

/// Redacts one line (registered secrets, bearer tokens, provider keys,
/// `password=` / `token:` pairs, JWTs, URL credentials, PEM keys).
#[must_use]
pub fn redact_line(text: &str) -> String {
    orkestr_security::redact::redact(text)
}

/// Renders ingest attributes as ` key=value` pairs; values of sensitive keys
/// are replaced entirely.
#[must_use]
pub fn render_attrs(attrs: &BTreeMap<String, String>) -> String {
    let mut out = String::new();
    for (k, v) in attrs {
        let k: String = k.chars().filter(|c| !c.is_whitespace() && *c != '=').collect();
        if k.is_empty() {
            continue;
        }
        let v = if orkestr_security::redact::is_sensitive_field(&k) {
            orkestr_security::redact::REDACTED.to_owned()
        } else {
            v.clone()
        };
        out.push(' ');
        out.push_str(&k);
        out.push('=');
        if v.contains(char::is_whitespace) {
            out.push('"');
            out.push_str(&v.replace('"', "'"));
            out.push('"');
        } else {
            out.push_str(&v);
        }
    }
    out
}

/// Guesses a level from common markers (`ERROR`, `level=warn`, `[DEBUG]`, ...).
#[must_use]
pub fn detect_level(line: &str) -> Option<LogLevel> {
    let head: String = line.chars().take(120).collect::<String>().to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| contains_word(&head, w));
    if has(&["error", "err", "fatal", "panic", "critical", "crit", "exception"]) {
        Some(LogLevel::Error)
    } else if has(&["warn", "warning"]) {
        Some(LogLevel::Warn)
    } else if has(&["info", "notice"]) {
        Some(LogLevel::Info)
    } else if has(&["debug"]) {
        Some(LogLevel::Debug)
    } else if has(&["trace"]) {
        Some(LogLevel::Trace)
    } else {
        None
    }
}

fn contains_word(hay: &str, word: &str) -> bool {
    hay.match_indices(word).any(|(i, _)| {
        let before = hay[..i].chars().next_back();
        let after = hay[i + word.len()..].chars().next();
        before.is_none_or(|c| !c.is_ascii_alphanumeric()) && after.is_none_or(|c| !c.is_ascii_alphanumeric())
    })
}

fn truncate(mut s: String) -> String {
    if s.len() > MAX_LINE_BYTES {
        let mut cut = MAX_LINE_BYTES;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push('\u{2026}');
    }
    s
}

/// Hook invoked for every stored line (alert evaluation).
pub type LineHook = Arc<dyn Fn(&LogLine, Option<ProjectId>) + Send + Sync>;

/// A bounded, redacting, broadcasting log buffer.
pub struct BufferedSource {
    info: LogSourceInfo,
    ring: Mutex<VecDeque<LogLine>>,
    capacity: usize,
    tx: broadcast::Sender<LogLine>,
    hook: Option<LineHook>,
    total: AtomicU64,
}

impl std::fmt::Debug for BufferedSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BufferedSource").field("id", &self.info.id).finish_non_exhaustive()
    }
}

impl BufferedSource {
    /// A source keeping `capacity` lines with a `tail_buffer` broadcast.
    #[must_use]
    pub fn new(info: LogSourceInfo, capacity: usize, tail_buffer: usize, hook: Option<LineHook>) -> Self {
        let (tx, _) = broadcast::channel(tail_buffer.max(1));
        Self { info, ring: Mutex::new(VecDeque::new()), capacity: capacity.max(1), tx, hook, total: AtomicU64::new(0) }
    }

    /// Descriptor.
    #[must_use]
    pub fn info(&self) -> &LogSourceInfo {
        &self.info
    }

    /// Lines accepted since start.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }

    /// Redacts and stores one raw line, then broadcasts it. Never blocks.
    pub fn push_raw(&self, ts: Option<Timestamp>, level: Option<LogLevel>, raw: &str) -> LogLine {
        let message = truncate(redact_line(raw.trim_end_matches(['\r', '\n'])));
        let level = level.or_else(|| detect_level(&message));
        let line =
            LogLine { ts: ts.unwrap_or_else(orkestr_domain::now), source_id: self.info.id.clone(), level, message };
        {
            let mut ring = self.ring.lock();
            if ring.len() == self.capacity {
                ring.pop_front();
            }
            ring.push_back(line.clone());
        }
        self.total.fetch_add(1, Ordering::Relaxed);
        if let Some(h) = &self.hook {
            h(&line, self.info.project_id);
        }
        // No receivers is fine; lagging receivers lose the oldest lines.
        let _ = self.tx.send(line.clone());
        line
    }

    /// Live tail with drop accounting.
    #[must_use]
    pub fn subscribe(&self) -> TailReceiver {
        TailReceiver { rx: self.tx.subscribe(), dropped: 0 }
    }

    /// Filtered snapshot (newest last).
    #[must_use]
    pub fn query_sync(&self, q: &LogQuery) -> (Vec<LogLine>, bool) {
        self.query_range(q, None)
    }

    /// Like [`Self::query_sync`] but also drops lines newer than `until`.
    #[must_use]
    pub fn query_range(&self, q: &LogQuery, until: Option<Timestamp>) -> (Vec<LogLine>, bool) {
        let limit = q.limit.unwrap_or(500).clamp(1, 10_000) as usize;
        let needle = q.contains.as_deref().map(str::to_lowercase);
        let ring = self.ring.lock();
        let mut out: Vec<LogLine> = Vec::new();
        let mut truncated = false;
        for l in ring.iter().rev() {
            if q.since.is_some_and(|s| l.ts < s) || until.is_some_and(|u| l.ts > u) {
                continue;
            }
            if let Some(min) = q.min_level {
                if l.level.is_none_or(|lv| lv < min) {
                    continue;
                }
            }
            if let Some(n) = &needle {
                if !l.message.to_lowercase().contains(n) {
                    continue;
                }
            }
            if out.len() == limit {
                truncated = true;
                break;
            }
            out.push(l.clone());
        }
        out.reverse();
        (out, truncated)
    }
}

#[async_trait]
impl LogSource for BufferedSource {
    fn id(&self) -> &str {
        &self.info.id
    }

    async fn query(&self, q: &LogQuery) -> Result<Vec<LogLine>, MonitorError> {
        Ok(self.query_sync(q).0)
    }

    fn tail(&self) -> broadcast::Receiver<LogLine> {
        self.tx.subscribe()
    }
}

/// A tail receiver that skips over lag and counts dropped lines.
#[derive(Debug)]
pub struct TailReceiver {
    rx: broadcast::Receiver<LogLine>,
    dropped: u64,
}

impl TailReceiver {
    /// Wraps a raw receiver (e.g. from [`LogSource::tail`]).
    #[must_use]
    pub fn new(rx: broadcast::Receiver<LogLine>) -> Self {
        Self { rx, dropped: 0 }
    }

    /// Next line; `None` when the source is gone.
    pub async fn recv(&mut self) -> Option<LogLine> {
        loop {
            match self.rx.recv().await {
                Ok(l) => return Some(l),
                Err(broadcast::error::RecvError::Lagged(n)) => self.dropped += n,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }

    /// Lines this receiver lost because it was too slow.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

/// Follows a file: reads appended bytes, survives truncation and rotation
/// (file replaced or shrunk → reopen from the start). Starts at the last
/// `backfill` bytes so a fresh source shows recent context.
pub fn spawn_file_tail(source: Arc<BufferedSource>, path: PathBuf, poll: Duration, cancel: CancellationToken) {
    tokio::spawn(async move {
        let mut state = TailState::default();
        loop {
            let src = Arc::clone(&source);
            let p = path.clone();
            let st = std::mem::take(&mut state);
            match tokio::task::spawn_blocking(move || read_new(&p, st, &src)).await {
                Ok(s) => state = s,
                Err(e) => tracing::warn!(error = %e, "file tail task failed"),
            }
            tokio::select! {
                () = cancel.cancelled() => break,
                () = tokio::time::sleep(poll) => {}
            }
        }
    });
}

const BACKFILL_BYTES: u64 = 64 * 1024;
const READ_CHUNK: u64 = 1024 * 1024;

const HEAD_BYTES: usize = 128;

#[derive(Debug, Default)]
struct TailState {
    pos: Option<u64>,
    partial: Vec<u8>,
    /// First bytes of the followed file: different bytes at the same path =
    /// the file was replaced (rename rotation). Inode / creation time are not
    /// reliable on Windows (file-system tunnelling keeps creation times).
    head: Vec<u8>,
}

fn read_head(f: &mut std::fs::File, n: usize) -> Vec<u8> {
    let mut buf = Vec::with_capacity(n);
    if f.seek(SeekFrom::Start(0)).is_ok() {
        let _ = f.take(n as u64).read_to_end(&mut buf);
    }
    buf
}

fn read_new(path: &std::path::Path, mut st: TailState, src: &BufferedSource) -> TailState {
    let Ok(mut f) = std::fs::File::open(path) else {
        // Missing (mid-rotation): keep the position until it reappears.
        return st;
    };
    let Ok(meta) = f.metadata() else { return st };
    let len = meta.len();
    let rotated = !st.head.is_empty() && read_head(&mut f, st.head.len()) != st.head;
    let start = match st.pos {
        None => len.saturating_sub(BACKFILL_BYTES),
        Some(p) if rotated || p > len => {
            st.partial.clear();
            0
        }
        Some(p) => p,
    };
    if rotated || st.head.len() < HEAD_BYTES {
        st.head = read_head(&mut f, HEAD_BYTES);
    }
    if start == len {
        st.pos = Some(len);
        return st;
    }
    let to_read = (len - start).min(READ_CHUNK);
    let mut buf = Vec::with_capacity(usize::try_from(to_read).unwrap_or(0));
    if f.seek(SeekFrom::Start(start)).is_err() || f.take(to_read).read_to_end(&mut buf).is_err() {
        return st;
    }
    let skip_first_partial = st.pos.is_none() && start > 0;
    st.pos = Some(start + buf.len() as u64);
    let mut data = std::mem::take(&mut st.partial);
    data.extend_from_slice(&buf);
    let mut lines: Vec<&[u8]> = data.split(|b| *b == b'\n').collect();
    let rest = lines.pop().unwrap_or_default().to_vec();
    if skip_first_partial && !lines.is_empty() {
        lines.remove(0);
    }
    for l in lines {
        let text = String::from_utf8_lossy(l);
        if !text.trim().is_empty() {
            src.push_raw(None, None, &text);
        }
    }
    st.partial = if rest.len() > MAX_LINE_BYTES * 4 { Vec::new() } else { rest };
    st
}

/// A deployment the monitor should follow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeploymentRef {
    /// Deployment.
    pub deployment_id: DeploymentId,
    /// Project.
    pub project_id: Option<ProjectId>,
    /// Display name (environment / version).
    pub name: String,
}

/// Deployment logs provided by the deploy service (WP4). The integrator wires
/// it to `DeployService` (`deploy.logs` semantics: line offsets).
#[async_trait]
pub trait DeploymentLogFeed: Send + Sync {
    /// Recent deployments (newest first).
    async fn recent(&self, limit: usize) -> Vec<DeploymentRef>;
    /// Lines from `offset` (already redacted by WP4; redacted again here).
    async fn read(&self, deployment_id: DeploymentId, offset: u64) -> Result<DeployLogsResult, String>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_protocol::methods::monitor::LogSourceKind;

    fn src(cap: usize, tail: usize) -> BufferedSource {
        BufferedSource::new(
            LogSourceInfo {
                id: "s".into(),
                name: "s".into(),
                kind: LogSourceKind::Ingest { stream: "s".into() },
                project_id: None,
            },
            cap,
            tail,
            None,
        )
    }

    #[test]
    fn levels_are_detected_on_word_boundaries() {
        assert_eq!(detect_level("2024 ERROR boom"), Some(LogLevel::Error));
        assert_eq!(detect_level("level=warn msg=x"), Some(LogLevel::Warn));
        assert_eq!(detect_level("[INFO] ok"), Some(LogLevel::Info));
        assert_eq!(detect_level("terror information"), None);
    }

    #[test]
    fn secrets_are_redacted_before_storage_and_broadcast() {
        let s = src(10, 10);
        let mut rx = s.subscribe();
        let l = s.push_raw(None, None, "login password=hunter2secret token: ghp_abcdefghijklmnopqrstuvwxyz0123456789");
        assert!(!l.message.contains("hunter2secret"), "{}", l.message);
        assert!(!l.message.contains("ghp_abcdefghijklmnopqrstuvwxyz0123456789"), "{}", l.message);
        let l2 = s.push_raw(None, None, "Authorization: Bearer abcdefghijklmnopqrstuvwxyz123456");
        assert!(!l2.message.contains("abcdefghijklmnopqrstuvwxyz123456"), "{}", l2.message);
        let (stored, _) = s.query_sync(&LogQuery::default());
        assert!(stored.iter().all(|l| !l.message.contains("hunter2secret")));
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let got = rt.block_on(rx.recv()).unwrap();
        assert!(!got.message.contains("hunter2secret"));
        let attrs: BTreeMap<String, String> =
            [("api_key".to_owned(), "AAAA-plain".to_owned()), ("user".to_owned(), "bob".to_owned())].into();
        let r = render_attrs(&attrs);
        assert!(r.contains("api_key=[REDACTED]") && r.contains("user=bob"), "{r}");
    }

    #[test]
    fn ring_bounds_and_query_filters() {
        let s = src(5, 4);
        for i in 0..8 {
            s.push_raw(
                None,
                if i % 2 == 0 { Some(LogLevel::Error) } else { Some(LogLevel::Info) },
                &format!("line {i}"),
            );
        }
        let (all, _) = s.query_sync(&LogQuery::default());
        assert_eq!(
            all.iter().map(|l| l.message.as_str()).collect::<Vec<_>>(),
            ["line 3", "line 4", "line 5", "line 6", "line 7"]
        );
        let (errs, _) = s.query_sync(&LogQuery { min_level: Some(LogLevel::Warn), ..LogQuery::default() });
        assert_eq!(errs.len(), 2);
        let (lim, trunc) = s.query_sync(&LogQuery { limit: Some(2), ..LogQuery::default() });
        assert!(trunc);
        assert_eq!(lim[1].message, "line 7");
        let (txt, _) = s.query_sync(&LogQuery { contains: Some("LINE 5".into()), ..LogQuery::default() });
        assert_eq!(txt.len(), 1);
    }

    #[tokio::test]
    async fn slow_tail_consumer_never_blocks_and_counts_drops() {
        let s = src(100_000, 16);
        let mut slow = s.subscribe();
        let started = std::time::Instant::now();
        for i in 0..20_000 {
            s.push_raw(None, None, &format!("l{i}"));
        }
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(s.total(), 20_000);
        let first = slow.recv().await.unwrap();
        assert!(slow.dropped() >= 20_000 - 16, "dropped {}", slow.dropped());
        assert_eq!(first.message, format!("l{}", 20_000 - 16));
    }

    #[tokio::test]
    async fn file_tail_follows_appends_truncation_and_rotation() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.log");
        std::fs::write(&path, "old line\n").unwrap();
        let s = Arc::new(src(1000, 100));
        let cancel = CancellationToken::new();
        spawn_file_tail(Arc::clone(&s), path.clone(), Duration::from_millis(30), cancel.clone());
        let wait = |n: &'static str| {
            let s = Arc::clone(&s);
            async move {
                for _ in 0..200 {
                    if s.query_sync(&LogQuery::default()).0.iter().any(|l| l.message == n) {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                panic!("line {n} never arrived: {:?}", s.query_sync(&LogQuery::default()).0);
            }
        };
        wait("old line").await;
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(b"second secret=topsecretvalue1\npart").unwrap();
        f.flush().unwrap();
        drop(f);
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(b"ial\n").unwrap();
        drop(f);
        wait("partial").await;
        assert!(s.query_sync(&LogQuery::default()).0.iter().all(|l| !l.message.contains("topsecretvalue1")));
        // Truncate (copytruncate rotation).
        std::fs::write(&path, "after truncate\n").unwrap();
        wait("after truncate").await;
        // Rename rotation: new file at the same path.
        std::fs::rename(&path, dir.path().join("app.log.1")).unwrap();
        std::fs::write(&path, "fresh file line that is long enough\n").unwrap();
        wait("fresh file line that is long enough").await;
        cancel.cancel();
    }
}
