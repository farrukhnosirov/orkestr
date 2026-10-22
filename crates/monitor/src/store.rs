//! Persistence. Tables are prefixed `monitor_`; migrations are composed by
//! `orkestr-core::migrations::all()` (never reorder, never edit an applied one).
//!
//! Rows carry the wire type as JSON (`spec` / `body`) plus the columns used
//! for filtering, so the schema stays small while the protocol evolves
//! additively.

use orkestr_domain::{AlertId, HealthCheckId};
use orkestr_protocol::methods::monitor::{Alert, CheckState, HealthCheck, LogSourceInfo};
use orkestr_storage::{Db, Migration};
use rusqlite::OptionalExtension;

use crate::alerts::AlertRule;
use crate::MonitorError;

/// Checks, alert rules, alerts and configured log sources.
pub const MONITOR_0001: Migration = Migration {
    id: "monitor_0001_checks_alerts_sources",
    owner: "monitor",
    up: "CREATE TABLE monitor_checks (
            id          TEXT PRIMARY KEY NOT NULL,
            project_id  TEXT,
            spec        TEXT NOT NULL,
            state       TEXT NOT NULL DEFAULT 'Unknown',
            since       TEXT,
            updated_at  TEXT NOT NULL
         );
         CREATE INDEX monitor_checks_project ON monitor_checks(project_id);
         CREATE TABLE monitor_alert_rules (
            id          TEXT PRIMARY KEY NOT NULL,
            spec        TEXT NOT NULL,
            updated_at  TEXT NOT NULL
         );
         CREATE TABLE monitor_alerts (
            id          TEXT PRIMARY KEY NOT NULL,
            project_id  TEXT,
            dedup_key   TEXT NOT NULL,
            state       TEXT NOT NULL,
            opened_at   TEXT NOT NULL,
            body        TEXT NOT NULL
         );
         CREATE INDEX monitor_alerts_key ON monitor_alerts(dedup_key, state);
         CREATE INDEX monitor_alerts_opened ON monitor_alerts(opened_at);
         CREATE TABLE monitor_log_sources (
            id          TEXT PRIMARY KEY NOT NULL,
            project_id  TEXT,
            spec        TEXT NOT NULL,
            created_at  TEXT NOT NULL
         );",
    down: "DROP TABLE IF EXISTS monitor_log_sources;
           DROP INDEX IF EXISTS monitor_alerts_opened;
           DROP INDEX IF EXISTS monitor_alerts_key;
           DROP TABLE IF EXISTS monitor_alerts;
           DROP TABLE IF EXISTS monitor_alert_rules;
           DROP INDEX IF EXISTS monitor_checks_project;
           DROP TABLE IF EXISTS monitor_checks;",
};

/// Migrations owned by this crate (owner `monitor`).
pub const MIGRATIONS: &[Migration] = &[MONITOR_0001];

/// [`MIGRATIONS`] as a vector.
#[must_use]
pub fn migrations() -> Vec<Migration> {
    MIGRATIONS.to_vec()
}

fn to_json<T: serde::Serialize>(v: &T) -> Result<String, MonitorError> {
    serde_json::to_string(v).map_err(|e| MonitorError::Internal(e.to_string()))
}

fn from_json<T: serde::de::DeserializeOwned>(s: &str) -> Option<T> {
    match serde_json::from_str(s) {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::warn!(error = %e, "skipping unreadable monitor row");
            None
        }
    }
}

fn state_name(s: CheckState) -> &'static str {
    match s {
        CheckState::Unknown => "Unknown",
        CheckState::Up => "Up",
        CheckState::Down => "Down",
    }
}

fn parse_state(s: &str) -> CheckState {
    match s {
        "Up" => CheckState::Up,
        "Down" => CheckState::Down,
        _ => CheckState::Unknown,
    }
}

/// A persisted check with its last known state.
#[derive(Debug, Clone)]
pub struct StoredCheck {
    /// Spec.
    pub check: HealthCheck,
    /// State.
    pub state: CheckState,
    /// State since.
    pub since: Option<orkestr_domain::Timestamp>,
}

/// Thin typed access to the monitor tables.
#[derive(Clone, Debug)]
pub struct MonitorStore {
    db: Db,
}

impl MonitorStore {
    /// Wraps the database.
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// All checks.
    pub async fn load_checks(&self) -> Result<Vec<StoredCheck>, MonitorError> {
        Ok(self
            .db
            .call(|c| {
                let mut st = c.prepare("SELECT spec, state, since FROM monitor_checks ORDER BY rowid")?;
                let rows = st
                    .query_map([], |r| {
                        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await?
            .into_iter()
            .filter_map(|(spec, state, since)| {
                Some(StoredCheck {
                    check: from_json(&spec)?,
                    state: parse_state(&state),
                    since: since.and_then(|s| s.parse().ok()),
                })
            })
            .collect())
    }

    /// Inserts or replaces a check spec (state preserved on update).
    pub async fn save_check(&self, check: &HealthCheck) -> Result<(), MonitorError> {
        let id = check.id.to_string();
        let project = check.project_id.map(|p| p.to_string());
        let spec = to_json(check)?;
        let now = orkestr_domain::now().to_rfc3339();
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO monitor_checks (id, project_id, spec, updated_at) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(id) DO UPDATE SET project_id = ?2, spec = ?3, updated_at = ?4",
                    rusqlite::params![id, project, spec, now],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// Persists a check's state.
    pub async fn save_check_state(
        &self,
        id: HealthCheckId,
        state: CheckState,
        since: Option<orkestr_domain::Timestamp>,
    ) -> Result<(), MonitorError> {
        let id = id.to_string();
        let since = since.map(|s| s.to_rfc3339());
        self.db
            .call(move |c| {
                c.execute(
                    "UPDATE monitor_checks SET state = ?2, since = ?3 WHERE id = ?1",
                    rusqlite::params![id, state_name(state), since],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// Deletes a check; `false` when missing.
    pub async fn delete_check(&self, id: HealthCheckId) -> Result<bool, MonitorError> {
        let id = id.to_string();
        let n = self.db.call(move |c| Ok(c.execute("DELETE FROM monitor_checks WHERE id = ?1", [id])?)).await?;
        Ok(n > 0)
    }

    /// Alert rules.
    pub async fn load_rules(&self) -> Result<Vec<AlertRule>, MonitorError> {
        Ok(self
            .db
            .call(|c| {
                let mut st = c.prepare("SELECT spec FROM monitor_alert_rules ORDER BY rowid")?;
                let rows = st.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await?
            .iter()
            .filter_map(|s| from_json(s))
            .collect())
    }

    /// Inserts or replaces a rule.
    pub async fn save_rule(&self, rule: &AlertRule) -> Result<(), MonitorError> {
        let id = rule.id.clone();
        let spec = to_json(rule)?;
        let now = orkestr_domain::now().to_rfc3339();
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO monitor_alert_rules (id, spec, updated_at) VALUES (?1, ?2, ?3)
                     ON CONFLICT(id) DO UPDATE SET spec = ?2, updated_at = ?3",
                    rusqlite::params![id, spec, now],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// Deletes a rule; `false` when missing.
    pub async fn delete_rule(&self, id: &str) -> Result<bool, MonitorError> {
        let id = id.to_owned();
        let n = self.db.call(move |c| Ok(c.execute("DELETE FROM monitor_alert_rules WHERE id = ?1", [id])?)).await?;
        Ok(n > 0)
    }

    /// Inserts or replaces an alert.
    pub async fn save_alert(&self, alert: &Alert, dedup_key: &str) -> Result<(), MonitorError> {
        let id = alert.id.to_string();
        let project = alert.project_id.map(|p| p.to_string());
        let key = dedup_key.to_owned();
        let state = format!("{:?}", alert.state);
        let opened = alert.opened_at.to_rfc3339();
        let body = to_json(alert)?;
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO monitor_alerts (id, project_id, dedup_key, state, opened_at, body)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT(id) DO UPDATE SET state = ?4, body = ?6",
                    rusqlite::params![id, project, key, state, opened, body],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// Unresolved alerts with their dedup keys.
    pub async fn load_open_alerts(&self) -> Result<Vec<(String, Alert)>, MonitorError> {
        Ok(self
            .db
            .call(|c| {
                let mut st = c.prepare(
                    "SELECT dedup_key, body FROM monitor_alerts WHERE state != 'Resolved' ORDER BY opened_at",
                )?;
                let rows = st
                    .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await?
            .into_iter()
            .filter_map(|(k, b)| Some((k, from_json(&b)?)))
            .collect())
    }

    /// Alerts, newest first.
    pub async fn list_alerts(
        &self,
        project: Option<orkestr_domain::ProjectId>,
        include_resolved: bool,
        limit: u32,
    ) -> Result<Vec<Alert>, MonitorError> {
        let project = project.map(|p| p.to_string());
        Ok(self
            .db
            .call(move |c| {
                let mut st = c.prepare(
                    "SELECT body FROM monitor_alerts
                     WHERE (?1 IS NULL OR project_id = ?1) AND (?2 OR state != 'Resolved')
                     ORDER BY opened_at DESC LIMIT ?3",
                )?;
                let rows = st
                    .query_map(rusqlite::params![project, include_resolved, limit], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await?
            .iter()
            .filter_map(|s| from_json(s))
            .collect())
    }

    /// One alert.
    pub async fn get_alert(&self, id: AlertId) -> Result<Option<Alert>, MonitorError> {
        let id = id.to_string();
        let body = self
            .db
            .call(move |c| {
                Ok(c.query_row("SELECT body FROM monitor_alerts WHERE id = ?1", [id], |r| r.get::<_, String>(0))
                    .optional()?)
            })
            .await?;
        Ok(body.and_then(|b| from_json(&b)))
    }

    /// Configured (persistent) log sources.
    pub async fn load_sources(&self) -> Result<Vec<LogSourceInfo>, MonitorError> {
        Ok(self
            .db
            .call(|c| {
                let mut st = c.prepare("SELECT spec FROM monitor_log_sources ORDER BY rowid")?;
                let rows = st.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await?
            .iter()
            .filter_map(|s| from_json(s))
            .collect())
    }

    /// Inserts or replaces a log source.
    pub async fn save_source(&self, info: &LogSourceInfo) -> Result<(), MonitorError> {
        let id = info.id.clone();
        let project = info.project_id.map(|p| p.to_string());
        let spec = to_json(info)?;
        let now = orkestr_domain::now().to_rfc3339();
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO monitor_log_sources (id, project_id, spec, created_at) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(id) DO UPDATE SET project_id = ?2, spec = ?3",
                    rusqlite::params![id, project, spec, now],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// Deletes a log source; `false` when missing.
    pub async fn delete_source(&self, id: &str) -> Result<bool, MonitorError> {
        let id = id.to_owned();
        let n = self.db.call(move |c| Ok(c.execute("DELETE FROM monitor_log_sources WHERE id = ?1", [id])?)).await?;
        Ok(n > 0)
    }
}
