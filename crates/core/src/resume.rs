//! Session resume summary (§57, RR-G9): what happened since the user last
//! looked — durable event counts by kind plus a few highlights.
//!
//! Generic on purpose: it only reads the event log, so it keeps working as
//! engines add event kinds. `Log` and `SettingsChanged` are excluded (noise).

use orkestr_domain::{ProjectId, Timestamp};
use orkestr_protocol::methods::system::{EventKindCount, ResumeSummary};
use orkestr_protocol::Event;
use orkestr_storage::rusqlite::params_from_iter;
use orkestr_storage::rusqlite::types::Value;
use orkestr_storage::{Db, StorageError, StorageResult};

/// Kinds excluded from the summary.
pub const IGNORED_KINDS: &[&str] = &["Log", "SettingsChanged"];
/// Kinds worth showing individually.
pub const HIGHLIGHT_KINDS: &[&str] = &[
    "BuildFailed",
    "TestRunFinished",
    "ReviewFinished",
    "ApprovalRequested",
    "ApprovalResolved",
    "EmergencyStop",
    "RollbackPerformed",
    "SnapshotCreated",
    "DecisionRecorded",
    "DeploymentFinished",
    "BudgetThreshold",
    "Notification",
    "ProjectCreated",
];
const MAX_HIGHLIGHTS: i64 = 8;

fn quoted(list: &[&str]) -> String {
    list.iter().map(|k| format!("'{k}'")).collect::<Vec<_>>().join(",")
}

/// Builds the summary for `[since, now]`, optionally for one project.
pub async fn summarize(
    db: &Db,
    since: Option<Timestamp>,
    project_id: Option<ProjectId>,
    previous_session_started_at: Option<Timestamp>,
    recovered_from_crash: bool,
) -> StorageResult<ResumeSummary> {
    let mut filter = format!("kind NOT IN ({})", quoted(IGNORED_KINDS));
    let mut args: Vec<Value> = Vec::new();
    if let Some(s) = since {
        filter.push_str(" AND ts >= ?");
        args.push(Value::Text(s.to_rfc3339()));
    }
    if let Some(p) = project_id {
        filter.push_str(" AND project_id = ?");
        args.push(Value::Text(p.to_string()));
    }
    let counts_sql = format!("SELECT kind, COUNT(*) FROM events WHERE {filter} GROUP BY kind ORDER BY 2 DESC, 1 ASC");
    // Task completions/failures are highlights too (payload `to`).
    let highlight_sql = format!(
        "SELECT doc FROM events WHERE {filter} AND (kind IN ({}) OR (kind = 'TaskStatusChanged' AND \
         json_extract(doc, '$.payload.to') IN ('Completed','Failed','Interrupted'))) ORDER BY seq DESC LIMIT {MAX_HIGHLIGHTS}",
        quoted(HIGHLIGHT_KINDS)
    );
    let (counts, docs) = db
        .call(move |c| {
            let mut stmt = c.prepare(&counts_sql)?;
            let counts = stmt
                .query_map(params_from_iter(args.iter()), |r| {
                    Ok(EventKindCount { kind: r.get(0)?, count: u64::try_from(r.get::<_, i64>(1)?).unwrap_or(0) })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            let mut stmt = c.prepare(&highlight_sql)?;
            let docs = stmt
                .query_map(params_from_iter(args.iter()), |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok((counts, docs))
        })
        .await?;
    let mut highlights = docs
        .iter()
        .map(|d| serde_json::from_str::<Event>(d).map_err(StorageError::from))
        .collect::<Result<Vec<_>, _>>()?;
    highlights.reverse();
    let total_events = counts.iter().map(|c| c.count).sum();
    Ok(ResumeSummary { since, previous_session_started_at, recovered_from_crash, counts, total_events, highlights })
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_events::{EventLog, SqliteEventLog};
    use orkestr_protocol::event::{EventKind, LogLevel, LogPayload, ProjectDeletedPayload};
    use orkestr_protocol::NewEvent;

    #[tokio::test]
    async fn counts_and_highlights() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path().join("r.db")).unwrap();
        db.migrate(crate::migrations::all()).await.unwrap();
        let log = SqliteEventLog::new(db.clone());
        let now = chrono::Utc::now();
        let old = now - chrono::Duration::days(3);
        let p1 = ProjectId::new();
        let p2 = ProjectId::new();
        let deleted = |p| EventKind::ProjectDeleted(ProjectDeletedPayload { project_id: p });
        let lg = EventKind::Log(LogPayload {
            level: LogLevel::Info,
            target: "t".into(),
            message: "m".into(),
            fields: serde_json::Value::Null,
        });
        let rows = [(1, old, deleted(p1)), (2, now, deleted(p1)), (3, now, deleted(p2)), (4, now, lg)];
        for (seq, ts, kind) in rows {
            log.append(&NewEvent::new(kind).into_event(seq, ts)).await.unwrap();
        }
        let since = Some(now - chrono::Duration::hours(1));
        let all = summarize(&db, since, None, None, false).await.unwrap();
        assert_eq!(all.total_events, 2, "old events and logs are excluded: {all:?}");
        assert_eq!(all.counts, vec![EventKindCount { kind: "ProjectDeleted".into(), count: 2 }]);
        let one = summarize(&db, since, Some(p1), None, true).await.unwrap();
        assert_eq!(one.total_events, 1);
        assert!(one.recovered_from_crash);
        let ever = summarize(&db, None, Some(p1), None, false).await.unwrap();
        assert_eq!(ever.total_events, 2);
        assert!(ever.highlights.is_empty(), "ProjectDeleted is not a highlight");
    }
}
