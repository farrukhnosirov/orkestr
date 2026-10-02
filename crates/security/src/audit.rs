//! Append-only audit log (permission model §8).
//!
//! Every permission decision of interest, approval resolution, rule change,
//! secret access and emergency control becomes an [`AuditEvent`] row in
//! `audit_events`. The table is append-only (SQLite triggers abort UPDATE and
//! DELETE) and rows form a blake3 hash chain (`hash = blake3(prev_hash ‖
//! canonical row)`), which makes tampering evident (not impossible).
//! Resources and details are redacted before they are written.

use orkestr_domain::{
    now, Actor, AuditEvent, AuditEventId, AuditOutcome, Capability, CorrelationId, ProjectId, Timestamp,
};
use orkestr_storage::rusqlite::{params, OptionalExtension};
use orkestr_storage::{Db, Migration, StorageError};
use serde::{Deserialize, Serialize};

use crate::error::SecurityResult;
use crate::redact::{redact, redact_json};

/// Hash of the (virtual) row before the first one.
pub const GENESIS_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Migration creating the audit table.
pub const MIGRATION: Migration = Migration {
    id: "security_0002_audit",
    owner: "security",
    up: r"
CREATE TABLE audit_events (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    ts TEXT NOT NULL,
    actor TEXT NOT NULL,
    action TEXT NOT NULL,
    capability TEXT,
    resource TEXT NOT NULL,
    project_id TEXT,
    outcome TEXT NOT NULL,
    details TEXT NOT NULL,
    correlation_id TEXT,
    prev_hash TEXT NOT NULL,
    hash TEXT NOT NULL
);
CREATE INDEX audit_events_project ON audit_events(project_id, seq);
CREATE INDEX audit_events_action ON audit_events(action, seq);
CREATE TRIGGER audit_events_no_update BEFORE UPDATE ON audit_events
BEGIN SELECT RAISE(ABORT, 'audit_events is append-only'); END;
CREATE TRIGGER audit_events_no_delete BEFORE DELETE ON audit_events
BEGIN SELECT RAISE(ABORT, 'audit_events is append-only'); END;
",
    down: r"
DROP TRIGGER IF EXISTS audit_events_no_delete;
DROP TRIGGER IF EXISTS audit_events_no_update;
DROP TABLE IF EXISTS audit_events;
",
};

/// An entry to append.
#[derive(Debug, Clone, PartialEq)]
pub struct NewAudit {
    /// Actor.
    pub actor: Actor,
    /// Action name (`permissions.check`, `secrets.read`, `emergency.stopAll`, …).
    pub action: String,
    /// Capability involved.
    pub capability: Option<Capability>,
    /// Resource (path, host, secret name — never a secret value). Redacted on append.
    pub resource: String,
    /// Project.
    pub project_id: Option<ProjectId>,
    /// Outcome.
    pub outcome: AuditOutcome,
    /// Details. Redacted on append.
    pub details: serde_json::Value,
    /// Correlation id; `None` uses the ambient request correlation.
    pub correlation_id: Option<CorrelationId>,
}

impl NewAudit {
    /// Minimal entry.
    #[must_use]
    pub fn new(actor: Actor, action: impl Into<String>, resource: impl Into<String>, outcome: AuditOutcome) -> Self {
        Self {
            actor,
            action: action.into(),
            capability: None,
            resource: resource.into(),
            project_id: None,
            outcome,
            details: serde_json::Value::Null,
            correlation_id: None,
        }
    }

    /// Sets the capability.
    #[must_use]
    pub fn capability(mut self, c: Capability) -> Self {
        self.capability = Some(c);
        self
    }

    /// Sets the project.
    #[must_use]
    pub fn project(mut self, p: Option<ProjectId>) -> Self {
        self.project_id = p;
        self
    }

    /// Sets details.
    #[must_use]
    pub fn details(mut self, d: serde_json::Value) -> Self {
        self.details = d;
        self
    }
}

/// A stored audit row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditRecord {
    /// Sequence (monotonic).
    pub seq: i64,
    /// The event.
    pub event: AuditEvent,
    /// Chain hash of this row.
    pub hash: String,
}

/// Query filter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuditQuery {
    /// Only this project.
    pub project_id: Option<ProjectId>,
    /// Only actions starting with this prefix (`secrets.`).
    pub action_prefix: Option<String>,
    /// Only rows with `seq < before_seq` (paging backwards).
    pub before_seq: Option<i64>,
    /// Max rows (default 100, max 1000).
    pub limit: Option<u32>,
}

/// The audit log.
#[derive(Debug, Clone)]
pub struct AuditLog {
    db: Db,
}

fn enum_str<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default()
}

#[allow(clippy::too_many_arguments)]
fn chain_hash(
    prev: &str,
    id: &str,
    ts: &str,
    actor: &str,
    action: &str,
    capability: &str,
    resource: &str,
    project: &str,
    outcome: &str,
    details: &str,
    correlation: &str,
) -> String {
    let mut h = blake3::Hasher::new();
    for part in [prev, id, ts, actor, action, capability, resource, project, outcome, details, correlation] {
        h.update(&(part.len() as u64).to_le_bytes());
        h.update(part.as_bytes());
    }
    h.finalize().to_hex().to_string()
}

impl AuditLog {
    /// Creates the log over a migrated database.
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Appends one entry (redacting resource and details).
    pub async fn append(&self, entry: NewAudit) -> SecurityResult<AuditEvent> {
        let correlation_id = entry.correlation_id.or_else(orkestr_events::current_correlation);
        let event = AuditEvent {
            id: AuditEventId::new(),
            ts: now(),
            actor: entry.actor,
            action: entry.action,
            capability: entry.capability,
            resource: redact(&entry.resource),
            project_id: entry.project_id,
            outcome: entry.outcome,
            details: redact_json(&entry.details),
            correlation_id,
        };
        let ev = event.clone();
        self.db
            .call(move |c| {
                let tx = c.transaction()?;
                let prev: String = tx
                    .query_row("SELECT hash FROM audit_events ORDER BY seq DESC LIMIT 1", [], |r| r.get(0))
                    .optional()?
                    .unwrap_or_else(|| GENESIS_HASH.to_owned());
                let id = ev.id.to_string();
                let ts = ev.ts.to_rfc3339();
                let actor = serde_json::to_string(&ev.actor)?;
                let capability = ev.capability.map(|c| enum_str(&c)).unwrap_or_default();
                let project = ev.project_id.map(|p| p.to_string()).unwrap_or_default();
                let outcome = enum_str(&ev.outcome);
                let details = serde_json::to_string(&ev.details)?;
                let correlation = ev.correlation_id.map(|c| c.to_string()).unwrap_or_default();
                let hash = chain_hash(
                    &prev, &id, &ts, &actor, &ev.action, &capability, &ev.resource, &project, &outcome, &details, &correlation,
                );
                tx.execute(
                    "INSERT INTO audit_events (id, ts, actor, action, capability, resource, project_id, outcome, details, correlation_id, prev_hash, hash)
                     VALUES (?1, ?2, ?3, ?4, NULLIF(?5, ''), ?6, NULLIF(?7, ''), ?8, ?9, NULLIF(?10, ''), ?11, ?12)",
                    params![id, ts, actor, ev.action, capability, ev.resource, project, outcome, details, correlation, prev, hash],
                )?;
                tx.commit()?;
                Ok(())
            })
            .await?;
        tracing::debug!(action = %event.action, outcome = ?event.outcome, "audit");
        Ok(event)
    }

    /// Appends, logging (not propagating) failures — for audit calls on
    /// paths where the primary operation already succeeded.
    pub async fn record(&self, entry: NewAudit) {
        if let Err(e) = self.append(entry).await {
            tracing::error!(error = %e, "failed to append audit event");
        }
    }

    /// Queries newest-first.
    pub async fn query(&self, q: AuditQuery) -> SecurityResult<Vec<AuditRecord>> {
        let limit = i64::from(q.limit.unwrap_or(100).clamp(1, 1000));
        let rows = self
            .db
            .call(move |c| {
                let mut sql = String::from(
                    "SELECT seq, id, ts, actor, action, capability, resource, project_id, outcome, details, correlation_id, hash FROM audit_events WHERE 1=1",
                );
                let mut args: Vec<Box<dyn orkestr_storage::rusqlite::ToSql>> = Vec::new();
                if let Some(p) = q.project_id {
                    sql.push_str(" AND project_id = ?");
                    args.push(Box::new(p.to_string()));
                }
                if let Some(prefix) = q.action_prefix.filter(|s| !s.is_empty()) {
                    sql.push_str(" AND substr(action, 1, length(?)) = ?");
                    args.push(Box::new(prefix.clone()));
                    args.push(Box::new(prefix));
                }
                if let Some(b) = q.before_seq {
                    sql.push_str(" AND seq < ?");
                    args.push(Box::new(b));
                }
                sql.push_str(" ORDER BY seq DESC LIMIT ?");
                args.push(Box::new(limit));
                let mut stmt = c.prepare(&sql)?;
                let refs: Vec<&dyn orkestr_storage::rusqlite::ToSql> = args.iter().map(AsRef::as_ref).collect();
                let rows = stmt.query_map(refs.as_slice(), |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, Option<String>>(5)?,
                        r.get::<_, String>(6)?,
                        r.get::<_, Option<String>>(7)?,
                        r.get::<_, String>(8)?,
                        r.get::<_, String>(9)?,
                        r.get::<_, Option<String>>(10)?,
                        r.get::<_, String>(11)?,
                    ))
                })?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row?);
                }
                Ok(out)
            })
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for (seq, id, ts, actor, action, capability, resource, project, outcome, details, correlation, hash) in rows {
            let parse = |what: &str, e: String| StorageError::Corrupt(format!("audit row {seq} {what}: {e}"));
            let event = AuditEvent {
                id: AuditEventId::parse(&id).map_err(|e| parse("id", e.to_string()))?,
                ts: Timestamp::from(
                    chrono::DateTime::parse_from_rfc3339(&ts)
                        .map_err(|e| parse("ts", e.to_string()))?
                        .with_timezone(&chrono::Utc),
                ),
                actor: serde_json::from_str(&actor).map_err(|e| parse("actor", e.to_string()))?,
                action,
                capability: capability
                    .map(|c| serde_json::from_value(serde_json::Value::String(c)))
                    .transpose()
                    .map_err(|e| parse("capability", e.to_string()))?,
                resource,
                project_id: project
                    .map(|p| ProjectId::parse(&p))
                    .transpose()
                    .map_err(|e| parse("project", e.to_string()))?,
                outcome: serde_json::from_value(serde_json::Value::String(outcome))
                    .map_err(|e| parse("outcome", e.to_string()))?,
                details: serde_json::from_str(&details).map_err(|e| parse("details", e.to_string()))?,
                correlation_id: correlation
                    .map(|c| CorrelationId::parse(&c))
                    .transpose()
                    .map_err(|e| parse("correlation", e.to_string()))?,
            };
            out.push(AuditRecord { seq, event, hash });
        }
        Ok(out)
    }

    /// Recomputes the hash chain over all rows. Returns the first broken
    /// `seq`, or `None` when the chain is intact.
    pub async fn verify_chain(&self) -> SecurityResult<Option<i64>> {
        let broken = self
            .db
            .call(|c| {
                let mut stmt = c.prepare(
                    "SELECT seq, id, ts, actor, action, COALESCE(capability, ''), resource, COALESCE(project_id, ''), outcome, details, COALESCE(correlation_id, ''), prev_hash, hash FROM audit_events ORDER BY seq ASC",
                )?;
                let mut rows = stmt.query([])?;
                let mut prev = GENESIS_HASH.to_owned();
                while let Some(r) = rows.next()? {
                    let seq: i64 = r.get(0)?;
                    let get = |i: usize| r.get::<_, String>(i);
                    let expected = chain_hash(
                        &prev, &get(1)?, &get(2)?, &get(3)?, &get(4)?, &get(5)?, &get(6)?, &get(7)?, &get(8)?, &get(9)?, &get(10)?,
                    );
                    let stored_prev: String = r.get(11)?;
                    let stored: String = r.get(12)?;
                    if stored_prev != prev || stored != expected {
                        return Ok(Some(seq));
                    }
                    prev = stored;
                }
                Ok(None)
            })
            .await?;
        Ok(broken)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn log() -> AuditLog {
        let db = Db::open_in_memory().unwrap();
        db.migrate(crate::migrations()).await.unwrap();
        AuditLog::new(db)
    }

    #[tokio::test]
    async fn append_query_and_chain() {
        let log = log().await;
        let p = ProjectId::new();
        for i in 0..5 {
            log.append(
                NewAudit::new(Actor::System, format!("test.{i}"), "res password=hunter2", AuditOutcome::Allowed)
                    .project(if i % 2 == 0 { Some(p) } else { None })
                    .capability(Capability::Write)
                    .details(serde_json::json!({"token": "abc", "n": i})),
            )
            .await
            .unwrap();
        }
        let all = log.query(AuditQuery::default()).await.unwrap();
        assert_eq!(all.len(), 5);
        assert!(all[0].seq > all[1].seq, "newest first");
        assert_eq!(all[0].event.resource, "res password=[REDACTED]");
        assert_eq!(all[0].event.details["token"], "[REDACTED]");
        let mine = log.query(AuditQuery { project_id: Some(p), ..Default::default() }).await.unwrap();
        assert_eq!(mine.len(), 3);
        let one = log.query(AuditQuery { action_prefix: Some("test.4".into()), ..Default::default() }).await.unwrap();
        assert_eq!(one.len(), 1);
        let page =
            log.query(AuditQuery { before_seq: Some(all[1].seq), limit: Some(2), ..Default::default() }).await.unwrap();
        assert_eq!(page.len(), 2);
        assert!(page.iter().all(|r| r.seq < all[1].seq));
        assert_eq!(log.verify_chain().await.unwrap(), None);
    }

    #[tokio::test]
    async fn table_is_append_only_and_tamper_evident() {
        let log = log().await;
        log.append(NewAudit::new(Actor::System, "a", "r", AuditOutcome::Denied)).await.unwrap();
        log.append(NewAudit::new(Actor::System, "b", "r", AuditOutcome::Denied)).await.unwrap();
        let upd = log.db.call(|c| Ok(c.execute("UPDATE audit_events SET action = 'x'", [])?)).await;
        assert!(upd.is_err(), "updates must be rejected");
        let del = log.db.call(|c| Ok(c.execute("DELETE FROM audit_events", [])?)).await;
        assert!(del.is_err(), "deletes must be rejected");
        // Simulate tampering by an attacker who drops the trigger.
        log.db
            .call(|c| {
                c.execute_batch(
                    "DROP TRIGGER audit_events_no_update; UPDATE audit_events SET action = 'x' WHERE seq = 1;",
                )?;
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(log.verify_chain().await.unwrap(), Some(1));
    }
}
