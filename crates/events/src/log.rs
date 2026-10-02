//! Persisted event log.

use async_trait::async_trait;
use orkestr_protocol::topic;
use orkestr_protocol::Event;
use orkestr_storage::rusqlite::{self, params_from_iter, types::Value};
use orkestr_storage::{Db, Migration, StorageError};

use crate::{EventError, EventResult};

/// Migrations owned by the events crate.
#[must_use]
pub fn events_migrations() -> Vec<Migration> {
    vec![Migration {
        id: "events_0001_log",
        owner: "events",
        up: "CREATE TABLE events (
                seq            INTEGER PRIMARY KEY NOT NULL,
                id             TEXT NOT NULL,
                ts             TEXT NOT NULL,
                kind           TEXT NOT NULL,
                project_id     TEXT,
                task_id        TEXT,
                agent_id       TEXT,
                correlation_id TEXT,
                topics         TEXT NOT NULL,
                doc            TEXT NOT NULL
             );
             CREATE INDEX events_project ON events(project_id, seq);
             CREATE INDEX events_correlation ON events(correlation_id);",
        down:
            "DROP INDEX IF EXISTS events_correlation; DROP INDEX IF EXISTS events_project; DROP TABLE IF EXISTS events;",
    }]
}

/// A range query over the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayQuery {
    /// First seq (inclusive).
    pub from_seq: u64,
    /// Last seq (inclusive); `None` = latest.
    pub to_seq: Option<u64>,
    /// Max events.
    pub limit: u32,
    /// Topic filter (`None` or containing `*` = everything).
    pub topics: Option<Vec<String>>,
    /// Take the newest `limit` events of the range instead of the oldest.
    pub newest_first: bool,
}

impl Default for ReplayQuery {
    fn default() -> Self {
        Self { from_seq: 1, to_seq: None, limit: 1000, topics: None, newest_first: false }
    }
}

/// One page of results, always in ascending `seq` order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ReplayPage {
    /// Events.
    pub events: Vec<Event>,
    /// More events exist in the requested range.
    pub has_more: bool,
}

/// Durable, append-only event storage.
#[async_trait]
pub trait EventLog: Send + Sync {
    /// Appends an event. `seq` must be greater than every stored seq.
    async fn append(&self, event: &Event) -> EventResult<()>;
    /// Highest stored seq (0 when empty).
    async fn last_seq(&self) -> EventResult<u64>;
    /// Range query.
    async fn range(&self, query: &ReplayQuery) -> EventResult<ReplayPage>;
    /// Deletes events with `seq < before`. Returns the number removed.
    async fn prune_before(&self, before: u64) -> EventResult<u64>;
}

fn topics_column(event: &Event) -> String {
    // Space-delimited with leading/trailing space so `instr(' t ')` matches whole topics.
    format!(" {} ", topic::event_topics(event).join(" "))
}

fn topic_filter(topics: &Option<Vec<String>>) -> EventResult<Option<Vec<String>>> {
    match topics {
        None => Ok(None),
        Some(t) if t.iter().any(|x| x == topic::ALL) => Ok(None),
        Some(t) => {
            if let Some(bad) = t.iter().find(|x| !topic::is_valid(x)) {
                return Err(EventError::Invalid(format!("invalid topic `{bad}`")));
            }
            Ok(Some(t.clone()))
        }
    }
}

/// SQLite implementation (table `events`, see [`events_migrations`]).
#[derive(Debug, Clone)]
pub struct SqliteEventLog {
    db: Db,
}

impl SqliteEventLog {
    /// Wraps a migrated database.
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

fn to_i64(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

#[async_trait]
impl EventLog for SqliteEventLog {
    async fn append(&self, event: &Event) -> EventResult<()> {
        let doc = serde_json::to_string(event).map_err(StorageError::from)?;
        let row = (
            to_i64(event.seq),
            event.id.to_string(),
            event.ts.to_rfc3339(),
            event.kind_name().to_owned(),
            event.project_id.map(|v| v.to_string()),
            event.task_id.map(|v| v.to_string()),
            event.agent_id.map(|v| v.to_string()),
            event.correlation_id.map(|v| v.to_string()),
            topics_column(event),
            doc,
        );
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO events (seq, id, ts, kind, project_id, task_id, agent_id, correlation_id, topics, doc)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    rusqlite::params![row.0, row.1, row.2, row.3, row.4, row.5, row.6, row.7, row.8, row.9],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    async fn last_seq(&self) -> EventResult<u64> {
        let v: i64 =
            self.db.call(|c| Ok(c.query_row("SELECT COALESCE(MAX(seq), 0) FROM events", [], |r| r.get(0))?)).await?;
        Ok(u64::try_from(v).unwrap_or(0))
    }

    async fn range(&self, query: &ReplayQuery) -> EventResult<ReplayPage> {
        let topics = topic_filter(&query.topics)?;
        let limit = query.limit.clamp(1, 10_000);
        let mut sql = String::from("SELECT doc FROM events WHERE seq >= ?");
        let mut args: Vec<Value> = vec![Value::Integer(to_i64(query.from_seq))];
        if let Some(to) = query.to_seq {
            sql.push_str(" AND seq <= ?");
            args.push(Value::Integer(to_i64(to)));
        }
        if let Some(topics) = topics {
            let ors: Vec<&str> = topics.iter().map(|_| "instr(topics, ?) > 0").collect();
            sql.push_str(&format!(" AND ({})", ors.join(" OR ")));
            for t in topics {
                args.push(Value::Text(format!(" {t} ")));
            }
        }
        sql.push_str(if query.newest_first { " ORDER BY seq DESC" } else { " ORDER BY seq ASC" });
        sql.push_str(" LIMIT ?");
        args.push(Value::Integer(i64::from(limit) + 1));
        let newest_first = query.newest_first;
        let mut docs: Vec<String> = self
            .db
            .call(move |c| {
                let mut stmt = c.prepare(&sql)?;
                let rows = stmt.query_map(params_from_iter(args.iter()), |r| r.get::<_, String>(0))?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .await?;
        let has_more = docs.len() > limit as usize;
        docs.truncate(limit as usize);
        if newest_first {
            docs.reverse();
        }
        let events = docs
            .iter()
            .map(|d| serde_json::from_str::<Event>(d).map_err(StorageError::from))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ReplayPage { events, has_more })
    }

    async fn prune_before(&self, before: u64) -> EventResult<u64> {
        let n = self.db.call(move |c| Ok(c.execute("DELETE FROM events WHERE seq < ?1", [to_i64(before)])?)).await?;
        Ok(n as u64)
    }
}

/// In-memory log for tests and ephemeral cores.
#[derive(Debug, Default)]
pub struct MemoryEventLog {
    events: tokio::sync::RwLock<Vec<Event>>,
}

impl MemoryEventLog {
    /// Empty log.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl EventLog for MemoryEventLog {
    async fn append(&self, event: &Event) -> EventResult<()> {
        self.events.write().await.push(event.clone());
        Ok(())
    }

    async fn last_seq(&self) -> EventResult<u64> {
        Ok(self.events.read().await.last().map_or(0, |e| e.seq))
    }

    async fn range(&self, query: &ReplayQuery) -> EventResult<ReplayPage> {
        let topics = topic_filter(&query.topics)?;
        let limit = query.limit.clamp(1, 10_000) as usize;
        let all = self.events.read().await;
        let mut matching: Vec<&Event> = all
            .iter()
            .filter(|e| e.seq >= query.from_seq && query.to_seq.is_none_or(|to| e.seq <= to))
            .filter(|e| topics.as_ref().is_none_or(|t| topic::matches(t, e)))
            .collect();
        let has_more = matching.len() > limit;
        if query.newest_first {
            let skip = matching.len().saturating_sub(limit);
            matching.drain(..skip);
        } else {
            matching.truncate(limit);
        }
        Ok(ReplayPage { events: matching.into_iter().cloned().collect(), has_more })
    }

    async fn prune_before(&self, before: u64) -> EventResult<u64> {
        let mut all = self.events.write().await;
        let len = all.len();
        all.retain(|e| e.seq >= before);
        Ok((len - all.len()) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_domain::{now, ProjectId};
    use orkestr_protocol::event::{EventKind, LogLevel, LogPayload, NewEvent};

    fn log_event(seq: u64, project: Option<ProjectId>) -> Event {
        let mut ev = NewEvent::new(EventKind::Log(LogPayload {
            level: LogLevel::Info,
            target: "test".into(),
            message: format!("event {seq}"),
            fields: serde_json::json!({ "n": seq }),
        }));
        if let Some(p) = project {
            ev = ev.project(p);
        }
        ev.into_event(seq, now())
    }

    async fn sqlite() -> (tempfile::TempDir, SqliteEventLog) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path().join("e.db")).unwrap();
        db.migrate(events_migrations()).await.unwrap();
        (dir, SqliteEventLog::new(db))
    }

    async fn exercise(log: &dyn EventLog) {
        let p = ProjectId::new();
        for seq in 1..=10 {
            log.append(&log_event(seq, if seq % 2 == 0 { Some(p) } else { None })).await.unwrap();
        }
        assert_eq!(log.last_seq().await.unwrap(), 10);

        let page = log.range(&ReplayQuery { from_seq: 3, to_seq: Some(6), ..Default::default() }).await.unwrap();
        assert_eq!(page.events.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![3, 4, 5, 6]);
        assert!(!page.has_more);

        let page = log.range(&ReplayQuery { limit: 3, ..Default::default() }).await.unwrap();
        assert_eq!(page.events.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![1, 2, 3]);
        assert!(page.has_more);

        let page = log.range(&ReplayQuery { limit: 3, newest_first: true, ..Default::default() }).await.unwrap();
        assert_eq!(page.events.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![8, 9, 10]);

        let page =
            log.range(&ReplayQuery { topics: Some(vec![topic::project(p)]), ..Default::default() }).await.unwrap();
        assert_eq!(page.events.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![2, 4, 6, 8, 10]);

        let page = log.range(&ReplayQuery { topics: Some(vec!["global".into()]), ..Default::default() }).await.unwrap();
        assert_eq!(page.events.len(), 5);

        assert!(log.range(&ReplayQuery { topics: Some(vec!["nope".into()]), ..Default::default() }).await.is_err());

        assert_eq!(log.prune_before(5).await.unwrap(), 4);
        let page = log.range(&ReplayQuery::default()).await.unwrap();
        assert_eq!(page.events.first().map(|e| e.seq), Some(5));
    }

    #[tokio::test]
    async fn sqlite_log_behaves() {
        let (_d, log) = sqlite().await;
        exercise(&log).await;
        // Events roundtrip fully.
        let page = log.range(&ReplayQuery { from_seq: 10, ..Default::default() }).await.unwrap();
        assert_eq!(page.events[0].kind_name(), "Log");
    }

    #[tokio::test]
    async fn memory_log_behaves() {
        exercise(&MemoryEventLog::new()).await;
    }
}
