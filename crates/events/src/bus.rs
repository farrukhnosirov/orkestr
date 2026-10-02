//! The event bus.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use orkestr_protocol::{topic, Event, NewEvent};
use tokio::sync::{broadcast, Mutex};

use crate::correlation::current_correlation;
use crate::log::{EventLog, ReplayPage, ReplayQuery};
use crate::{EventError, EventResult};

/// Default broadcast buffer (events) per bus. Slow subscribers that fall
/// further behind receive [`broadcast::error::RecvError::Lagged`] and should
/// re-sync via [`EventBus::replay`].
pub const DEFAULT_CAPACITY: usize = 4096;

struct Inner {
    tx: broadcast::Sender<Arc<Event>>,
    /// Serializes seq assignment + persistence + broadcast so that seq order,
    /// log order and delivery order are identical.
    publish_lock: Mutex<u64>,
    last_seq: AtomicU64,
    log: Option<Arc<dyn EventLog>>,
}

/// Cloneable handle to the bus.
#[derive(Clone)]
pub struct EventBus {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for EventBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventBus")
            .field("last_seq", &self.last_seq())
            .field("persistent", &self.inner.log.is_some())
            .field("subscribers", &self.inner.tx.receiver_count())
            .finish()
    }
}

impl EventBus {
    /// A bus without persistence (seq starts at 1).
    #[must_use]
    pub fn in_memory(capacity: usize) -> Self {
        Self::build(capacity, None, 0)
    }

    /// A bus persisting every non-ephemeral event to `log`; seq continues after
    /// the last persisted event.
    pub async fn with_log(log: Arc<dyn EventLog>, capacity: usize) -> EventResult<Self> {
        let last = log.last_seq().await?;
        Ok(Self::build(capacity, Some(log), last))
    }

    fn build(capacity: usize, log: Option<Arc<dyn EventLog>>, last: u64) -> Self {
        let (tx, _) = broadcast::channel(capacity.max(16));
        Self { inner: Arc::new(Inner { tx, publish_lock: Mutex::new(last), last_seq: AtomicU64::new(last), log }) }
    }

    /// Publishes an event: assigns `seq`, `id`, `ts` (and the ambient
    /// correlation id when none is set), persists it (unless ephemeral) and
    /// broadcasts it. If persistence fails the event is not broadcast and the
    /// seq is not consumed.
    pub async fn publish(&self, mut new: NewEvent) -> EventResult<Arc<Event>> {
        if new.correlation_id.is_none() {
            new.correlation_id = current_correlation();
        }
        let mut seq_guard = self.inner.publish_lock.lock().await;
        let seq = *seq_guard + 1;
        let event = new.into_event(seq, chrono::Utc::now());
        if let Some(log) = &self.inner.log {
            if !event.kind.is_ephemeral() {
                log.append(&event).await?;
            }
        }
        *seq_guard = seq;
        self.inner.last_seq.store(seq, Ordering::Release);
        let event = Arc::new(event);
        // No receivers is fine.
        let _ = self.inner.tx.send(Arc::clone(&event));
        drop(seq_guard);
        tracing::trace!(seq, kind = event.kind_name(), "event published");
        Ok(event)
    }

    /// Subscribes to topics (see [`orkestr_protocol::topic`]).
    pub fn subscribe<I, S>(&self, topics: I) -> EventResult<Subscription>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut sub = Subscription { rx: self.inner.tx.subscribe(), topics: BTreeSet::new() };
        sub.add_topics(topics)?;
        Ok(sub)
    }

    /// Subscribes to every event.
    #[must_use]
    pub fn subscribe_all(&self) -> Subscription {
        let mut topics = BTreeSet::new();
        topics.insert(topic::ALL.to_owned());
        Subscription { rx: self.inner.tx.subscribe(), topics }
    }

    /// Last assigned seq (0 when nothing was published).
    #[must_use]
    pub fn last_seq(&self) -> u64 {
        self.inner.last_seq.load(Ordering::Acquire)
    }

    /// Whether events are persisted.
    #[must_use]
    pub fn is_persistent(&self) -> bool {
        self.inner.log.is_some()
    }

    /// Queries the persisted log. Fails with `Invalid` on a non-persistent bus.
    pub async fn replay(&self, query: &ReplayQuery) -> EventResult<ReplayPage> {
        match &self.inner.log {
            Some(log) => log.range(query).await,
            None => Err(EventError::Invalid("event bus has no persistent log".into())),
        }
    }

    /// Number of live subscriptions.
    #[must_use]
    pub fn subscriber_count(&self) -> usize {
        self.inner.tx.receiver_count()
    }
}

/// Outcome of [`Subscription::recv`].
#[derive(Debug, Clone, PartialEq)]
pub enum Received {
    /// A matching event.
    Event(Arc<Event>),
    /// The subscriber fell behind and `n` events were dropped; re-sync via replay.
    Lagged(u64),
}

/// A topic-filtered receiver.
pub struct Subscription {
    rx: broadcast::Receiver<Arc<Event>>,
    topics: BTreeSet<String>,
}

impl std::fmt::Debug for Subscription {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Subscription").field("topics", &self.topics).finish_non_exhaustive()
    }
}

impl Subscription {
    /// Adds topics (validated).
    pub fn add_topics<I, S>(&mut self, topics: I) -> EventResult<()>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let topics: Vec<String> = topics.into_iter().map(Into::into).collect();
        if let Some(bad) = topics.iter().find(|t| !topic::is_valid(t)) {
            return Err(EventError::Invalid(format!("invalid topic `{bad}`")));
        }
        self.topics.extend(topics);
        Ok(())
    }

    /// Removes topics.
    pub fn remove_topics<I, S>(&mut self, topics: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for t in topics {
            self.topics.remove(t.as_ref());
        }
    }

    /// Current topics.
    #[must_use]
    pub fn topics(&self) -> Vec<String> {
        self.topics.iter().cloned().collect()
    }

    /// Whether an event matches this subscription.
    #[must_use]
    pub fn matches(&self, event: &Event) -> bool {
        let topics: Vec<&str> = self.topics.iter().map(String::as_str).collect();
        topic::matches(&topics, event)
    }

    /// Waits for the next matching event. Returns `None` when the bus is gone.
    pub async fn recv(&mut self) -> Option<Received> {
        loop {
            match self.rx.recv().await {
                Ok(ev) if self.matches(&ev) => return Some(Received::Event(ev)),
                Ok(_) => continue,
                Err(broadcast::error::RecvError::Lagged(n)) => return Some(Received::Lagged(n)),
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::{events_migrations, MemoryEventLog, SqliteEventLog};
    use crate::{new_correlation, with_correlation};
    use orkestr_domain::{now, Project, ProjectId, TerminalId};
    use orkestr_protocol::event::{EventKind, LogLevel, LogPayload, ProjectPayload, TerminalOutputPayload};

    fn log_kind(msg: &str) -> EventKind {
        EventKind::Log(LogPayload {
            level: LogLevel::Info,
            target: "t".into(),
            message: msg.into(),
            fields: serde_json::Value::Null,
        })
    }

    async fn next_event(sub: &mut Subscription) -> Arc<Event> {
        match tokio::time::timeout(std::time::Duration::from_secs(2), sub.recv()).await.unwrap().unwrap() {
            Received::Event(e) => e,
            Received::Lagged(n) => panic!("lagged {n}"),
        }
    }

    #[tokio::test]
    async fn seq_is_monotonic_and_delivery_ordered() {
        let bus = EventBus::in_memory(1024);
        let mut sub = bus.subscribe_all();
        let mut handles = Vec::new();
        for i in 0..50 {
            let bus = bus.clone();
            handles
                .push(tokio::spawn(async move { bus.publish(NewEvent::new(log_kind(&i.to_string()))).await.unwrap() }));
        }
        for h in handles {
            h.await.unwrap();
        }
        let mut last = 0;
        for _ in 0..50 {
            let ev = next_event(&mut sub).await;
            assert_eq!(ev.seq, last + 1, "delivery order must equal seq order");
            last = ev.seq;
        }
        assert_eq!(bus.last_seq(), 50);
    }

    #[tokio::test]
    async fn topic_filtering() {
        let bus = EventBus::in_memory(64);
        let p = ProjectId::new();
        let mut project_sub = bus.subscribe([topic::project(p)]).unwrap();
        let mut global_sub = bus.subscribe([topic::GLOBAL]).unwrap();
        assert!(bus.subscribe(["bad topic"]).is_err());

        bus.publish(NewEvent::new(log_kind("project scoped")).project(p)).await.unwrap();
        bus.publish(NewEvent::new(log_kind("global"))).await.unwrap();

        let ev = next_event(&mut project_sub).await;
        assert_eq!(ev.project_id, Some(p));
        let ev = next_event(&mut global_sub).await;
        assert!(ev.project_id.is_none());
        // project_sub must not receive the global one.
        assert!(tokio::time::timeout(std::time::Duration::from_millis(50), project_sub.recv()).await.is_err());

        project_sub.remove_topics([topic::project(p)]);
        project_sub.add_topics([topic::GLOBAL]).unwrap();
        bus.publish(NewEvent::new(log_kind("g2"))).await.unwrap();
        assert!(next_event(&mut project_sub).await.project_id.is_none());
    }

    #[tokio::test]
    async fn persistent_bus_resumes_seq_and_skips_ephemeral() {
        let dir = tempfile::tempdir().unwrap();
        let db = orkestr_storage::Db::open(dir.path().join("e.db")).unwrap();
        db.migrate(events_migrations()).await.unwrap();
        let log = Arc::new(SqliteEventLog::new(db.clone()));
        let bus = EventBus::with_log(log.clone(), 64).await.unwrap();
        let project = Project::new("P", now());
        bus.publish(NewEvent::new(EventKind::ProjectCreated(ProjectPayload { project }))).await.unwrap();
        bus.publish(NewEvent::new(EventKind::TerminalOutput(TerminalOutputPayload {
            terminal_id: TerminalId::new(),
            data: "ls\r\n".into(),
            offset: None,
        })))
        .await
        .unwrap();
        bus.publish(NewEvent::new(log_kind("after"))).await.unwrap();
        assert_eq!(bus.last_seq(), 3);
        let page = bus.replay(&ReplayQuery::default()).await.unwrap();
        assert_eq!(page.events.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![1, 3]);

        // "Restart": a new bus over the same log continues the sequence.
        let bus2 = EventBus::with_log(log, 64).await.unwrap();
        assert_eq!(bus2.last_seq(), 3);
        let ev = bus2.publish(NewEvent::new(log_kind("next"))).await.unwrap();
        assert_eq!(ev.seq, 4);
    }

    #[tokio::test]
    async fn ambient_correlation_is_applied() {
        let bus = EventBus::with_log(Arc::new(MemoryEventLog::new()), 64).await.unwrap();
        let id = new_correlation();
        let ev = with_correlation(id, bus.publish(NewEvent::new(log_kind("x")))).await.unwrap();
        assert_eq!(ev.correlation_id, Some(id));
        let ev = bus.publish(NewEvent::new(log_kind("y"))).await.unwrap();
        assert!(ev.correlation_id.is_none());
    }

    #[tokio::test]
    async fn lagging_subscriber_is_told() {
        let bus = EventBus::in_memory(16);
        let mut sub = bus.subscribe_all();
        for i in 0..40 {
            bus.publish(NewEvent::new(log_kind(&i.to_string()))).await.unwrap();
        }
        assert!(matches!(sub.recv().await, Some(Received::Lagged(_))));
    }
}
