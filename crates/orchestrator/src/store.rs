//! SQLite persistence of Manager conversations, messages, plans and runs.
//! Documents are stored as JSON next to indexed columns. Only visible text
//! reaches this module (no hidden reasoning exists in the provider types).

use orkestr_domain::{now, ConversationId, ManagerPlanId, ManagerRunId, ProjectId, Timestamp};
use orkestr_protocol::methods::manager::{
    ConversationSummary, ManagerChatMessage, ManagerPlan, ManagerRun, ManagerRunStatus, PlanStatus,
};
use orkestr_storage::rusqlite::{params, OptionalExtension};
use orkestr_storage::{Db, StorageError};

use crate::error::{OrchResult, OrchestratorError};

fn enum_str<T: serde::Serialize>(v: &T) -> Result<String, StorageError> {
    match serde_json::to_value(v)? {
        serde_json::Value::String(s) => Ok(s),
        other => Ok(other.to_string()),
    }
}

fn parse_ts(s: &str) -> Result<Timestamp, StorageError> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|t| t.with_timezone(&chrono::Utc))
        .map_err(|e| StorageError::Corrupt(format!("bad timestamp `{s}`: {e}")))
}

fn parse_doc<T: serde::de::DeserializeOwned>(doc: &str) -> Result<T, StorageError> {
    serde_json::from_str(doc).map_err(StorageError::from)
}

/// Persistence facade (cheap to clone).
#[derive(Debug, Clone)]
pub struct ManagerStore {
    db: Db,
}

impl ManagerStore {
    /// Wraps a migrated database.
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    // ------------------------------------------------------------------
    // conversations
    // ------------------------------------------------------------------

    /// The active conversation of a project (created on first use) and its start time.
    pub async fn active_conversation(&self, project: ProjectId) -> OrchResult<(ConversationId, Timestamp)> {
        let pid = project.to_string();
        let (id, started) = self
            .db
            .call(move |c| {
                let existing: Option<(String, String)> = c
                    .query_row(
                        "SELECT id, started_at FROM manager_conversations WHERE project_id = ?1 AND active = 1
                         ORDER BY started_at DESC LIMIT 1",
                        [&pid],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?;
                if let Some(row) = existing {
                    return Ok(row);
                }
                let id = ConversationId::new().to_string();
                let at = now().to_rfc3339();
                c.execute(
                    "INSERT INTO manager_conversations (id, project_id, active, started_at, message_count)
                     VALUES (?1, ?2, 1, ?3, 0)",
                    params![id, pid, at],
                )?;
                Ok((id, at))
            })
            .await?;
        let id = ConversationId::parse(&id).map_err(|e| OrchestratorError::Internal(e.to_string()))?;
        Ok((id, parse_ts(&started)?))
    }

    /// Stores a message (and counts it on its conversation).
    pub async fn add_message(&self, m: &ManagerChatMessage) -> OrchResult<()> {
        let doc = serde_json::to_string(m)?;
        let role = enum_str(&m.role)?;
        let m = m.clone();
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO manager_messages (id, conversation_id, project_id, role, created_at, doc)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT(id) DO UPDATE SET doc = excluded.doc",
                    params![
                        m.id,
                        m.conversation_id.to_string(),
                        m.project_id.to_string(),
                        role,
                        m.created_at.to_rfc3339(),
                        doc
                    ],
                )?;
                c.execute(
                    "UPDATE manager_conversations SET message_count =
                        (SELECT COUNT(*) FROM manager_messages WHERE conversation_id = ?1) WHERE id = ?1",
                    [m.conversation_id.to_string()],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// The newest `limit` messages of a conversation, oldest first.
    pub async fn messages(&self, conversation: ConversationId, limit: u32) -> OrchResult<Vec<ManagerChatMessage>> {
        let cid = conversation.to_string();
        let docs: Vec<String> = self
            .db
            .call(move |c| {
                let mut stmt = c.prepare(
                    "SELECT doc FROM manager_messages WHERE conversation_id = ?1 ORDER BY seq DESC LIMIT ?2",
                )?;
                let rows = stmt.query_map(params![cid, i64::from(limit)], |r| r.get::<_, String>(0))?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .await?;
        let mut out = docs.iter().map(|d| parse_doc(d)).collect::<Result<Vec<ManagerChatMessage>, _>>()?;
        out.reverse();
        Ok(out)
    }

    /// Time of the newest message of a project.
    pub async fn last_message_at(&self, project: ProjectId) -> OrchResult<Option<Timestamp>> {
        let pid = project.to_string();
        let at: Option<String> = self
            .db
            .call(move |c| {
                Ok(c.query_row("SELECT MAX(created_at) FROM manager_messages WHERE project_id = ?1", [pid], |r| {
                    r.get::<_, Option<String>>(0)
                })?)
            })
            .await?;
        at.map(|s| parse_ts(&s)).transpose().map_err(Into::into)
    }

    /// Archives a conversation with its summary.
    pub async fn archive_conversation(&self, conversation: ConversationId, summary: String) -> OrchResult<()> {
        let cid = conversation.to_string();
        let at = now().to_rfc3339();
        self.db
            .call(move |c| {
                c.execute(
                    "UPDATE manager_conversations SET active = 0, ended_at = ?2, summary = ?3 WHERE id = ?1",
                    params![cid, at, summary],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// Summaries of archived conversations, newest first.
    pub async fn summaries(&self, project: ProjectId, limit: u32) -> OrchResult<Vec<ConversationSummary>> {
        let pid = project.to_string();
        let rows: Vec<(String, String, i64, String, String)> = self
            .db
            .call(move |c| {
                let mut stmt = c.prepare(
                    "SELECT id, COALESCE(summary, ''), message_count, started_at, COALESCE(ended_at, started_at)
                     FROM manager_conversations WHERE project_id = ?1 AND active = 0
                     ORDER BY ended_at DESC LIMIT ?2",
                )?;
                let rows = stmt.query_map(params![pid, i64::from(limit)], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
                })?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .await?;
        rows.into_iter()
            .map(|(id, summary, count, started, ended)| {
                Ok(ConversationSummary {
                    conversation_id: ConversationId::parse(&id)
                        .map_err(|e| OrchestratorError::Internal(e.to_string()))?,
                    project_id: project,
                    summary,
                    message_count: u32::try_from(count).unwrap_or(0),
                    started_at: parse_ts(&started)?,
                    ended_at: parse_ts(&ended)?,
                })
            })
            .collect()
    }

    // ------------------------------------------------------------------
    // plans
    // ------------------------------------------------------------------

    /// Inserts or replaces a plan.
    pub async fn save_plan(&self, plan: &ManagerPlan) -> OrchResult<()> {
        let doc = serde_json::to_string(plan)?;
        let status = enum_str(&plan.status)?;
        let p = plan.clone();
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO manager_plans (id, project_id, status, created_at, updated_at, doc)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT(id) DO UPDATE SET status = excluded.status, updated_at = excluded.updated_at,
                        doc = excluded.doc",
                    params![
                        p.id.to_string(),
                        p.project_id.to_string(),
                        status,
                        p.created_at.to_rfc3339(),
                        p.updated_at.to_rfc3339(),
                        doc
                    ],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// One plan.
    pub async fn plan(&self, id: ManagerPlanId) -> OrchResult<ManagerPlan> {
        let pid = id.to_string();
        let doc: Option<String> = self
            .db
            .call(move |c| {
                Ok(c.query_row("SELECT doc FROM manager_plans WHERE id = ?1", [pid], |r| r.get(0)).optional()?)
            })
            .await?;
        let doc = doc.ok_or_else(|| OrchestratorError::NotFound(format!("plan {id}")))?;
        Ok(parse_doc(&doc)?)
    }

    /// Plans of a project, newest first, optionally only in some statuses.
    pub async fn plans(
        &self,
        project: ProjectId,
        statuses: Option<Vec<PlanStatus>>,
        limit: u32,
    ) -> OrchResult<Vec<ManagerPlan>> {
        let pid = project.to_string();
        let wanted: Option<Vec<String>> =
            statuses.map(|v| v.iter().map(enum_str).collect::<Result<Vec<_>, _>>()).transpose()?;
        let docs: Vec<String> = self
            .db
            .call(move |c| {
                let mut stmt = c.prepare(
                    "SELECT status, doc FROM manager_plans WHERE project_id = ?1 ORDER BY created_at DESC LIMIT ?2",
                )?;
                let rows =
                    stmt.query_map(params![pid, i64::from(limit) * 4], |r| Ok((r.get::<_, String>(0)?, r.get(1)?)))?;
                let mut out = Vec::new();
                for row in rows {
                    let (status, doc): (String, String) = row?;
                    if wanted.as_ref().is_none_or(|w| w.contains(&status)) {
                        out.push(doc);
                    }
                    if out.len() >= limit as usize {
                        break;
                    }
                }
                Ok(out)
            })
            .await?;
        docs.iter().map(|d| parse_doc(d).map_err(Into::into)).collect()
    }

    // ------------------------------------------------------------------
    // runs
    // ------------------------------------------------------------------

    /// Inserts or replaces a run.
    pub async fn save_run(&self, run: &ManagerRun) -> OrchResult<()> {
        let doc = serde_json::to_string(run)?;
        let status = enum_str(&run.status)?;
        let r = run.clone();
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO manager_runs (id, project_id, plan_id, status, started_at, updated_at, doc)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                     ON CONFLICT(id) DO UPDATE SET status = excluded.status, updated_at = excluded.updated_at,
                        doc = excluded.doc",
                    params![
                        r.id.to_string(),
                        r.project_id.to_string(),
                        r.plan_id.to_string(),
                        status,
                        r.started_at.to_rfc3339(),
                        r.updated_at.to_rfc3339(),
                        doc
                    ],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// One run.
    pub async fn run(&self, id: ManagerRunId) -> OrchResult<ManagerRun> {
        let rid = id.to_string();
        let doc: Option<String> = self
            .db
            .call(move |c| {
                Ok(c.query_row("SELECT doc FROM manager_runs WHERE id = ?1", [rid], |r| r.get(0)).optional()?)
            })
            .await?;
        let doc = doc.ok_or_else(|| OrchestratorError::NotFound(format!("run {id}")))?;
        Ok(parse_doc(&doc)?)
    }

    /// Runs, newest first, filtered by project and status.
    pub async fn runs(
        &self,
        project: Option<ProjectId>,
        statuses: Option<Vec<ManagerRunStatus>>,
        limit: u32,
    ) -> OrchResult<Vec<ManagerRun>> {
        let pid = project.map(|p| p.to_string());
        let wanted: Option<Vec<String>> =
            statuses.map(|v| v.iter().map(enum_str).collect::<Result<Vec<_>, _>>()).transpose()?;
        let docs: Vec<String> = self
            .db
            .call(move |c| {
                let mut stmt = c.prepare(
                    "SELECT status, doc FROM manager_runs WHERE (?1 IS NULL OR project_id = ?1)
                     ORDER BY started_at DESC",
                )?;
                let rows = stmt.query_map(params![pid], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
                let mut out = Vec::new();
                for row in rows {
                    let (status, doc) = row?;
                    if wanted.as_ref().is_none_or(|w| w.contains(&status)) {
                        out.push(doc);
                    }
                    if out.len() >= limit as usize {
                        break;
                    }
                }
                Ok(out)
            })
            .await?;
        docs.iter().map(|d| parse_doc(d).map_err(Into::into)).collect()
    }
}
