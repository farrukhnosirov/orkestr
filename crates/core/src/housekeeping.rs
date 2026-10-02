//! Background housekeeping of a running core (Phase 0 review follow-ups,
//! §12, §27, §50, RR-A11, RR-G13).
//!
//! * **Event `seq` high-water mark.** The bus assigns `seq` to every event
//!   but only persists durable kinds, and retention pruning deletes old rows,
//!   so `MAX(seq)` of the log can be *lower* than numbers already handed to
//!   clients. [`SeqHighWater`] persists a reservation in `core_meta`
//!   (`events_seq_reserved`): at start the bus continues after
//!   `max(log max, reservation)`, a new block of [`SEQ_RESERVE_BLOCK`] is
//!   reserved immediately and extended whenever fewer than half of it is
//!   left; a clean shutdown records the exact last seq. Hence a `seq` is
//!   never reused, even after a crash (a crash only leaves a gap).
//! * **Retention.** [`prune_events`] deletes durable events older than
//!   `settings.eventRetentionDays` (0 = keep forever) except approvals,
//!   decisions, emergency stops and rollbacks ([`RETAINED_KINDS`]), in
//!   batches so the single DB connection is never held for long.
//! * **Session history** for `system.resumeSummary` (`session_started_at`).
//! * **Node heartbeat**: samples resources every `node_sample_interval` and
//!   publishes ephemeral `NodeStatus` events on `node:<id>`.
//!
//! ## Atomicity of the event store
//!
//! `EventBus::publish` holds one async mutex across seq assignment, the
//! single-row `INSERT` into `events` (atomic in SQLite/WAL; the row carries
//! its own seq, so a crash can never leave a half-written event) and the
//! broadcast. A failed insert neither consumes the seq nor broadcasts. Rows
//! are never updated; pruning deletes whole rows. The high-water reservation
//! is written with its own statement *before* seqs from the new block are
//! handed out.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use orkestr_events::{EventLog, EventResult, ReplayPage, ReplayQuery};
use orkestr_protocol::event::{EventKind, NodeStatusPayload};
use orkestr_protocol::{Event, NewEvent};
use orkestr_storage::rusqlite::{params, OptionalExtension};
use orkestr_storage::{Db, StorageResult};
use tokio_util::sync::CancellationToken;

use crate::services::Services;

/// Size of a seq reservation block.
pub const SEQ_RESERVE_BLOCK: u64 = 100_000;
const SEQ_KEY: &str = "events_seq_reserved";
const SESSION_KEY: &str = "session_started_at";
/// Durable kinds never removed by retention (audit trail, RR-G13,
/// event-model §4.2 "Forever"). Names of kinds that other areas add later
/// (`Secret*`, `PermissionRuleChanged`) are listed up front on purpose.
pub const RETAINED_KINDS: &[&str] = &[
    "ApprovalRequested",
    "ApprovalResolved",
    "DecisionRecorded",
    "EmergencyStop",
    "RollbackPerformed",
    "SnapshotCreated",
    "ProjectDeleted",
    "SecretChanged",
    "SecretAccessed",
    "PermissionRuleChanged",
];
const PRUNE_BATCH: i64 = 2_000;
/// First retention run after start.
pub const PRUNE_FIRST_AFTER: Duration = Duration::from_secs(30);
/// Retention interval.
pub const PRUNE_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

async fn meta_get(db: &Db, key: &'static str) -> StorageResult<Option<String>> {
    db.call(move |c| Ok(c.query_row("SELECT value FROM core_meta WHERE key = ?1", [key], |r| r.get(0)).optional()?))
        .await
}

async fn meta_set(db: &Db, key: &'static str, value: String) -> StorageResult<()> {
    db.call(move |c| {
        c.execute(
            "INSERT INTO core_meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    })
    .await
}

/// Persistent seq reservation (see module docs).
#[derive(Debug)]
pub struct SeqHighWater {
    db: Db,
    reserved: AtomicU64,
}

impl SeqHighWater {
    /// Loads the stored reservation (0 when none).
    pub async fn load(db: Db) -> StorageResult<Self> {
        let stored = meta_get(&db, SEQ_KEY).await?.and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
        Ok(Self { db, reserved: AtomicU64::new(stored) })
    }

    /// Current reservation: every seq handed out so far is `<=` this value.
    #[must_use]
    pub fn reserved(&self) -> u64 {
        self.reserved.load(Ordering::Acquire)
    }

    /// Reserves `[base+1, base+BLOCK]` and persists it.
    pub async fn reserve_from(&self, base: u64) -> StorageResult<()> {
        let next = base.saturating_add(SEQ_RESERVE_BLOCK);
        meta_set(&self.db, SEQ_KEY, next.to_string()).await?;
        self.reserved.fetch_max(next, Ordering::AcqRel);
        Ok(())
    }

    /// Extends the reservation when less than half of the block is left.
    pub async fn maybe_extend(&self, last_seq: u64) -> StorageResult<bool> {
        if last_seq.saturating_add(SEQ_RESERVE_BLOCK / 2) >= self.reserved() {
            self.reserve_from(last_seq).await?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Clean shutdown: records the exact last seq so the next session continues without a gap.
    pub async fn finish(&self, last_seq: u64) -> StorageResult<()> {
        meta_set(&self.db, SEQ_KEY, last_seq.to_string()).await?;
        self.reserved.store(last_seq, Ordering::Release);
        Ok(())
    }
}

/// Event log wrapper whose `last_seq` never goes below `floor`, so the bus
/// resumes after the persisted high-water mark.
pub struct SeqFloorLog {
    inner: Arc<dyn EventLog>,
    floor: u64,
}

impl SeqFloorLog {
    /// Wraps `inner`.
    #[must_use]
    pub fn new(inner: Arc<dyn EventLog>, floor: u64) -> Self {
        Self { inner, floor }
    }
}

#[async_trait]
impl EventLog for SeqFloorLog {
    async fn append(&self, event: &Event) -> EventResult<()> {
        self.inner.append(event).await
    }
    async fn last_seq(&self) -> EventResult<u64> {
        Ok(self.inner.last_seq().await?.max(self.floor))
    }
    async fn range(&self, query: &ReplayQuery) -> EventResult<ReplayPage> {
        self.inner.range(query).await
    }
    async fn prune_before(&self, before: u64) -> EventResult<u64> {
        self.inner.prune_before(before).await
    }
}

/// Deletes durable events older than `retention_days` (0 = keep forever),
/// keeping [`RETAINED_KINDS`]. Returns the number of deleted events.
pub async fn prune_events(db: &Db, retention_days: u32, now: chrono::DateTime<chrono::Utc>) -> StorageResult<u64> {
    if retention_days == 0 {
        return Ok(0);
    }
    let cutoff = (now - chrono::Duration::days(i64::from(retention_days))).to_rfc3339();
    let keep = RETAINED_KINDS.iter().map(|k| format!("'{k}'")).collect::<Vec<_>>().join(",");
    let sql = format!(
        "DELETE FROM events WHERE seq IN (SELECT seq FROM events WHERE ts < ?1 AND kind NOT IN ({keep}) ORDER BY seq LIMIT ?2)"
    );
    let mut total = 0u64;
    loop {
        let (sql, cutoff) = (sql.clone(), cutoff.clone());
        let n = db.call(move |c| Ok(c.execute(&sql, params![cutoff, PRUNE_BATCH])?)).await?;
        total += n as u64;
        if (n as i64) < PRUNE_BATCH {
            break;
        }
        tokio::task::yield_now().await;
    }
    Ok(total)
}

/// Records the start of this session; returns the previous session's start.
pub async fn rotate_session(
    db: &Db,
    now: chrono::DateTime<chrono::Utc>,
) -> StorageResult<Option<chrono::DateTime<chrono::Utc>>> {
    let previous = meta_get(db, SESSION_KEY)
        .await?
        .and_then(|v| chrono::DateTime::parse_from_rfc3339(&v).ok())
        .map(|d| d.with_timezone(&chrono::Utc));
    meta_set(db, SESSION_KEY, now.to_rfc3339()).await?;
    Ok(previous)
}

async fn heartbeat(services: &Arc<Services>) {
    let monitor = Arc::clone(&services.nodes);
    let Ok(sample) = tokio::task::spawn_blocking(move || monitor.sample()).await else { return };
    let node = services.nodes.local_node();
    let event =
        NewEvent::new(EventKind::NodeStatus(NodeStatusPayload { node, sample: Some(sample) })).node(services.node_id);
    if let Err(e) = services.events.publish(event).await {
        tracing::debug!(error = %e, "failed to publish node heartbeat");
    }
}

async fn run_retention(services: &Arc<Services>) {
    let days = services.settings.get().event_retention_days;
    match prune_events(&services.db, days, chrono::Utc::now()).await {
        Ok(0) => tracing::debug!(retention_days = days, "event retention: nothing to prune"),
        Ok(n) => tracing::info!(pruned = n, retention_days = days, "event retention pruned old events"),
        Err(e) => tracing::warn!(error = %e, "event retention failed"),
    }
}

/// Spawns the housekeeping loop; it stops when `shutdown` is cancelled.
pub fn spawn(
    services: Arc<Services>,
    shutdown: CancellationToken,
    sample_interval: Duration,
    prefetch_after: Option<Duration>,
) -> tokio::task::JoinHandle<()> {
    if let Some(delay) = prefetch_after {
        let (services, shutdown) = (Arc::clone(&services), shutdown.clone());
        tokio::spawn(async move {
            tokio::select! {
                () = shutdown.cancelled() => {},
                () = tokio::time::sleep(delay) => {
                    let monitor = Arc::clone(&services.nodes);
                    let _ = tokio::task::spawn_blocking(move || monitor.info(false)).await;
                }
            }
        });
    }
    tokio::spawn(async move {
        let mut seq_tick = tokio::time::interval(Duration::from_secs(1));
        let mut sample_tick = tokio::time::interval(sample_interval);
        sample_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let prune_first = tokio::time::Instant::now() + PRUNE_FIRST_AFTER;
        let mut prune_tick = tokio::time::interval_at(prune_first, PRUNE_EVERY);
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                _ = seq_tick.tick() => {
                    if let Err(e) = services.seq_high_water.maybe_extend(services.events.last_seq()).await {
                        tracing::error!(error = %e, "failed to extend the event seq reservation");
                    }
                }
                _ = sample_tick.tick() => {
                    // Nobody listening → skip the (tens of ms) process scan.
                    if services.events.subscriber_count() > 0 {
                        heartbeat(&services).await;
                    }
                }
                _ = prune_tick.tick() => run_retention(&services).await,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_events::{EventBus, SqliteEventLog};
    use orkestr_protocol::event::{ApprovalResolvedPayload, LogLevel, LogPayload};

    async fn db() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path().join("h.db")).unwrap();
        db.migrate(crate::migrations::all()).await.unwrap();
        (dir, db)
    }

    fn log_event() -> NewEvent {
        NewEvent::new(EventKind::Log(LogPayload {
            level: LogLevel::Info,
            target: "t".into(),
            message: "m".into(),
            fields: serde_json::Value::Null,
        }))
    }

    async fn bus(db: &Db, hw: &SeqHighWater) -> EventBus {
        let log: Arc<dyn EventLog> = Arc::new(SqliteEventLog::new(db.clone()));
        let bus = EventBus::with_log(Arc::new(SeqFloorLog::new(log, hw.reserved())), 64).await.unwrap();
        hw.reserve_from(bus.last_seq()).await.unwrap();
        bus
    }

    #[tokio::test]
    async fn seq_is_never_reused_after_crash_or_prune() {
        let (_dir, db) = db().await;
        // Session 1: durable + ephemeral events, then a "crash" (no finish()).
        let hw = SeqHighWater::load(db.clone()).await.unwrap();
        let b = bus(&db, &hw).await;
        b.publish(log_event()).await.unwrap();
        let ephemeral = NewEvent::new(EventKind::TerminalOutput(orkestr_protocol::event::TerminalOutputPayload {
            terminal_id: orkestr_domain::TerminalId::new(),
            data: "x".into(),
            offset: None,
        }));
        let last = b.publish(ephemeral).await.unwrap().seq;
        assert_eq!(last, 2);
        drop(b);
        // Session 2 after a crash: continues after the reservation, never at 2.
        let hw = SeqHighWater::load(db.clone()).await.unwrap();
        let b = bus(&db, &hw).await;
        let first = b.publish(log_event()).await.unwrap().seq;
        assert!(first > last, "seq {first} reused");
        assert_eq!(first, SEQ_RESERVE_BLOCK + 1);
        // Clean shutdown records the exact value; the next session has no gap.
        hw.finish(b.last_seq()).await.unwrap();
        drop(b);
        // Even if every event is pruned, seq continues.
        db.call(|c| Ok(c.execute("DELETE FROM events", [])?)).await.unwrap();
        let hw = SeqHighWater::load(db.clone()).await.unwrap();
        let b = bus(&db, &hw).await;
        assert_eq!(b.publish(log_event()).await.unwrap().seq, first + 1);
    }

    #[tokio::test]
    async fn reservation_extends_before_exhaustion() {
        let (_dir, db) = db().await;
        let hw = SeqHighWater::load(db).await.unwrap();
        hw.reserve_from(0).await.unwrap();
        assert!(!hw.maybe_extend(10).await.unwrap());
        assert!(hw.maybe_extend(SEQ_RESERVE_BLOCK / 2 + 1).await.unwrap());
        assert_eq!(hw.reserved(), SEQ_RESERVE_BLOCK / 2 + 1 + SEQ_RESERVE_BLOCK);
    }

    #[tokio::test]
    async fn retention_keeps_audit_kinds_and_recent_events() {
        let (_dir, db) = db().await;
        let log = SqliteEventLog::new(db.clone());
        let now = chrono::Utc::now();
        let old = now - chrono::Duration::days(200);
        let approval = orkestr_domain::ApprovalId::new();
        let kinds = [
            (1, old, log_event().kind),
            (
                2,
                old,
                EventKind::ApprovalResolved(ApprovalResolvedPayload {
                    approval_id: approval,
                    status: orkestr_domain::ApprovalStatus::Approved,
                    resolved_by: None,
                }),
            ),
            (3, now, log_event().kind),
        ];
        for (seq, ts, kind) in kinds {
            log.append(&NewEvent::new(kind).into_event(seq, ts)).await.unwrap();
        }
        assert_eq!(prune_events(&db, 0, now).await.unwrap(), 0, "0 = keep forever");
        assert_eq!(prune_events(&db, 180, now).await.unwrap(), 1);
        let left: Vec<i64> = db
            .call(|c| {
                let mut s = c.prepare("SELECT seq FROM events ORDER BY seq")?;
                let r = s.query_map([], |r| r.get(0))?.collect::<Result<Vec<_>, _>>()?;
                Ok(r)
            })
            .await
            .unwrap();
        assert_eq!(left, vec![2, 3]);
    }

    #[tokio::test]
    async fn session_rotation() {
        let (_dir, db) = db().await;
        let t1 = chrono::Utc::now();
        assert_eq!(rotate_session(&db, t1).await.unwrap(), None);
        let prev = rotate_session(&db, t1 + chrono::Duration::hours(1)).await.unwrap().unwrap();
        assert_eq!(prev.timestamp(), t1.timestamp());
    }
}
