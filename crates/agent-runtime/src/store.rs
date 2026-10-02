//! SQLite persistence of agent instances, executions, transcript messages,
//! tool calls and handoffs. Documents are stored as JSON next to indexed
//! columns; every text that reaches this module is already redacted.

use orkestr_domain::{
    AgentId, AgentInstance, AgentState, Execution, ExecutionId, ProjectId, TaskId, Timestamp, ToolCall, ToolCallId,
    ToolCallStatus, UsageSummary,
};
use orkestr_protocol::event::MessageRole;
use orkestr_protocol::methods::agents::{
    AgentStateCount, AgentStats, AgentToolCallView, AgentTranscriptMessage, Handoff,
};
use orkestr_storage::rusqlite::{params, OptionalExtension};
use orkestr_storage::{Db, StorageError};

use crate::error::{AgentError, AgentResult};
use crate::state::ALL_STATES;

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

/// Filter of [`AgentStore::list`].
#[derive(Debug, Clone, Default)]
pub struct AgentFilter {
    /// Project.
    pub project_id: Option<ProjectId>,
    /// Task.
    pub task_id: Option<TaskId>,
    /// Only agents in these states.
    pub states: Option<Vec<AgentState>>,
    /// Max rows (default 500).
    pub limit: Option<u32>,
}

/// Persistence facade (cheap to clone).
#[derive(Debug, Clone)]
pub struct AgentStore {
    db: Db,
}

impl AgentStore {
    /// Wraps a migrated database.
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    // ------------------------------------------------------------------
    // instances
    // ------------------------------------------------------------------

    /// Inserts or replaces an instance.
    pub async fn save(&self, agent: &AgentInstance) -> AgentResult<()> {
        let doc = serde_json::to_string(agent)?;
        let state = enum_str(&agent.state)?;
        let a = agent.clone();
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO agent_instances (id, project_id, task_id, template_id, state, created_at, updated_at, doc)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                     ON CONFLICT(id) DO UPDATE SET task_id = excluded.task_id, state = excluded.state,
                        updated_at = excluded.updated_at, doc = excluded.doc",
                    params![
                        a.id.to_string(),
                        a.project_id.to_string(),
                        a.task_id.map(|t| t.to_string()),
                        a.template_id.map(|t| t.to_string()),
                        state,
                        a.created_at.to_rfc3339(),
                        chrono::Utc::now().to_rfc3339(),
                        doc
                    ],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// Loads an instance.
    pub async fn get(&self, id: AgentId) -> AgentResult<AgentInstance> {
        let doc: Option<String> = self
            .db
            .call(move |c| {
                Ok(c.query_row("SELECT doc FROM agent_instances WHERE id = ?1", [id.to_string()], |r| r.get(0))
                    .optional()?)
            })
            .await?;
        let doc = doc.ok_or_else(|| AgentError::NotFound(format!("agent {id}")))?;
        Ok(serde_json::from_str(&doc)?)
    }

    /// Lists instances, newest first.
    pub async fn list(&self, filter: AgentFilter) -> AgentResult<Vec<AgentInstance>> {
        let states: Option<Vec<String>> =
            filter.states.map(|s| s.iter().map(enum_str).collect::<Result<Vec<_>, _>>()).transpose()?;
        let docs: Vec<String> = self
            .db
            .call(move |c| {
                let mut sql = String::from("SELECT doc FROM agent_instances WHERE 1 = 1");
                let mut args: Vec<String> = Vec::new();
                if let Some(p) = filter.project_id {
                    args.push(p.to_string());
                    sql.push_str(&format!(" AND project_id = ?{}", args.len()));
                }
                if let Some(t) = filter.task_id {
                    args.push(t.to_string());
                    sql.push_str(&format!(" AND task_id = ?{}", args.len()));
                }
                if let Some(states) = &states {
                    if states.is_empty() {
                        return Ok(Vec::new());
                    }
                    let mut marks = Vec::new();
                    for s in states {
                        args.push(s.clone());
                        marks.push(format!("?{}", args.len()));
                    }
                    sql.push_str(&format!(" AND state IN ({})", marks.join(", ")));
                }
                sql.push_str(&format!(
                    " ORDER BY created_at DESC, id DESC LIMIT {}",
                    filter.limit.unwrap_or(500).min(5000)
                ));
                let mut stmt = c.prepare(&sql)?;
                let rows = stmt.query_map(orkestr_storage::rusqlite::params_from_iter(args.iter()), |r| r.get(0))?;
                let mut out = Vec::new();
                for r in rows {
                    out.push(r?);
                }
                Ok(out)
            })
            .await?;
        docs.iter().map(|d| serde_json::from_str(d).map_err(AgentError::from)).collect()
    }

    /// Number of instances of a template in a project (display-name numbering).
    pub async fn count_for_template(&self, project: ProjectId, template: &str) -> AgentResult<u32> {
        let template = template.to_owned();
        let n: i64 = self
            .db
            .call(move |c| {
                Ok(c.query_row(
                    "SELECT COUNT(*) FROM agent_instances WHERE project_id = ?1 AND template_id = ?2",
                    params![project.to_string(), template],
                    |r| r.get(0),
                )?)
            })
            .await?;
        Ok(u32::try_from(n).unwrap_or(u32::MAX))
    }

    // ------------------------------------------------------------------
    // executions
    // ------------------------------------------------------------------

    /// Inserts or replaces an execution.
    pub async fn save_execution(&self, e: &Execution) -> AgentResult<()> {
        let doc = serde_json::to_string(e)?;
        let status = enum_str(&e.status)?;
        let (id, agent, task, started) =
            (e.id.to_string(), e.agent_id.to_string(), e.task_id.to_string(), e.started_at);
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO agent_executions (id, agent_id, task_id, status, started_at, doc) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT(id) DO UPDATE SET status = excluded.status, doc = excluded.doc",
                    params![id, agent, task, status, started.to_rfc3339(), doc],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// Executions of an agent, oldest first.
    pub async fn executions(&self, agent: AgentId) -> AgentResult<Vec<Execution>> {
        let docs: Vec<String> = self
            .db
            .call(move |c| {
                let mut stmt =
                    c.prepare("SELECT doc FROM agent_executions WHERE agent_id = ?1 ORDER BY started_at, id")?;
                let rows = stmt.query_map([agent.to_string()], |r| r.get(0))?;
                let mut out = Vec::new();
                for r in rows {
                    out.push(r?);
                }
                Ok(out)
            })
            .await?;
        docs.iter().map(|d| serde_json::from_str(d).map_err(AgentError::from)).collect()
    }

    // ------------------------------------------------------------------
    // transcript
    // ------------------------------------------------------------------

    /// Appends a transcript message; returns its seq.
    pub async fn add_message(
        &self,
        id: &str,
        agent: AgentId,
        execution: Option<ExecutionId>,
        role: MessageRole,
        text: &str,
        tool_call_ids: &[ToolCallId],
    ) -> AgentResult<u64> {
        let (id, text) = (id.to_owned(), text.to_owned());
        let role = enum_str(&role)?;
        let calls = serde_json::to_string(tool_call_ids)?;
        let seq: i64 = self
            .db
            .call(move |c| {
                c.execute(
                    "INSERT INTO agent_messages (id, agent_id, execution_id, role, content, tool_call_ids, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        id,
                        agent.to_string(),
                        execution.map(|e| e.to_string()),
                        role,
                        text,
                        calls,
                        chrono::Utc::now().to_rfc3339()
                    ],
                )?;
                Ok(c.last_insert_rowid())
            })
            .await?;
        Ok(u64::try_from(seq).unwrap_or(0))
    }

    /// The newest `limit` messages of an agent, oldest first.
    pub async fn messages(&self, agent: AgentId, limit: u32) -> AgentResult<Vec<AgentTranscriptMessage>> {
        type Row = (i64, String, Option<String>, String, String, String, String);
        let rows: Vec<Row> = self
            .db
            .call(move |c| {
                let mut stmt = c.prepare(
                    "SELECT seq, id, execution_id, role, content, tool_call_ids, created_at FROM agent_messages
                     WHERE agent_id = ?1 ORDER BY seq DESC LIMIT ?2",
                )?;
                let rows = stmt.query_map(params![agent.to_string(), i64::from(limit)], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))
                })?;
                let mut out = Vec::new();
                for r in rows {
                    out.push(r?);
                }
                Ok(out)
            })
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for (seq, id, exec, role, text, calls, at) in rows.into_iter().rev() {
            out.push(AgentTranscriptMessage {
                id,
                seq: u64::try_from(seq).unwrap_or(0),
                agent_id: agent,
                execution_id: exec.map(|e| e.parse()).transpose().map_err(|e| AgentError::Internal(format!("{e}")))?,
                role: serde_json::from_value(serde_json::Value::String(role))?,
                text,
                tool_call_ids: serde_json::from_str(&calls)?,
                created_at: parse_ts(&at)?,
            });
        }
        Ok(out)
    }

    // ------------------------------------------------------------------
    // tool calls
    // ------------------------------------------------------------------

    /// Inserts or updates a tool call (+ result preview and duration).
    pub async fn save_tool_call(
        &self,
        call: &ToolCall,
        result: Option<&str>,
        duration_ms: Option<u64>,
    ) -> AgentResult<()> {
        let doc = serde_json::to_string(call)?;
        let status = enum_str(&call.status)?;
        let call = call.clone();
        let result = result.map(str::to_owned);
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO agent_tool_calls (id, agent_id, execution_id, task_id, tool, status, started_at, duration_ms, result, doc)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                     ON CONFLICT(id) DO UPDATE SET status = excluded.status, duration_ms = excluded.duration_ms,
                        result = COALESCE(excluded.result, agent_tool_calls.result), doc = excluded.doc",
                    params![
                        call.id.to_string(),
                        call.agent_id.to_string(),
                        call.execution_id.to_string(),
                        call.task_id.map(|t| t.to_string()),
                        call.tool,
                        status,
                        call.started_at.to_rfc3339(),
                        duration_ms.map(|d| i64::try_from(d).unwrap_or(i64::MAX)),
                        result,
                        doc
                    ],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// The newest `limit` tool calls of an agent, oldest first.
    pub async fn tool_calls(&self, agent: AgentId, limit: u32) -> AgentResult<Vec<AgentToolCallView>> {
        let rows: Vec<(String, Option<String>, Option<i64>)> = self
            .db
            .call(move |c| {
                let mut stmt = c.prepare(
                    "SELECT doc, result, duration_ms FROM agent_tool_calls WHERE agent_id = ?1 ORDER BY seq DESC LIMIT ?2",
                )?;
                let rows = stmt.query_map(params![agent.to_string(), i64::from(limit)], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })?;
                let mut out = Vec::new();
                for r in rows {
                    out.push(r?);
                }
                Ok(out)
            })
            .await?;
        rows.into_iter()
            .rev()
            .map(|(doc, result, d)| {
                Ok(AgentToolCallView {
                    tool_call: serde_json::from_str(&doc)?,
                    result,
                    duration_ms: d.and_then(|d| u64::try_from(d).ok()),
                })
            })
            .collect()
    }

    // ------------------------------------------------------------------
    // handoffs
    // ------------------------------------------------------------------

    /// Persists a handoff.
    pub async fn add_handoff(&self, agent: AgentId, h: &Handoff) -> AgentResult<()> {
        let doc = serde_json::to_string(h)?;
        let reason = enum_str(&h.reason)?;
        let (id, task, at) = (h.handoff_id.clone(), h.task.task_id.to_string(), h.created_at.to_rfc3339());
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO agent_handoffs (id, agent_id, task_id, reason, created_at, doc) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![id, agent.to_string(), task, reason, at, doc],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// Handoffs of an agent, newest first.
    pub async fn handoffs(&self, agent: AgentId) -> AgentResult<Vec<Handoff>> {
        self.handoff_docs("agent_id", agent.to_string(), 100).await
    }

    /// The newest handoff of a task (any agent).
    pub async fn latest_task_handoff(&self, task: TaskId) -> AgentResult<Option<Handoff>> {
        Ok(self.handoff_docs("task_id", task.to_string(), 1).await?.into_iter().next())
    }

    async fn handoff_docs(&self, column: &'static str, value: String, limit: u32) -> AgentResult<Vec<Handoff>> {
        let docs: Vec<String> = self
            .db
            .call(move |c| {
                let sql = format!(
                    "SELECT doc FROM agent_handoffs WHERE {column} = ?1 ORDER BY created_at DESC, id DESC LIMIT {limit}"
                );
                let mut stmt = c.prepare(&sql)?;
                let rows = stmt.query_map([value], |r| r.get(0))?;
                let mut out = Vec::new();
                for r in rows {
                    out.push(r?);
                }
                Ok(out)
            })
            .await?;
        docs.iter().map(|d| serde_json::from_str(d).map_err(AgentError::from)).collect()
    }

    // ------------------------------------------------------------------
    // stats
    // ------------------------------------------------------------------

    /// Aggregate counters.
    pub async fn stats(&self, project: Option<ProjectId>, active: u32) -> AgentResult<AgentStats> {
        let agents =
            self.list(AgentFilter { project_id: project, limit: Some(5000), ..AgentFilter::default() }).await?;
        let mut usage = UsageSummary::default();
        let mut turns = 0u64;
        let mut by_state = Vec::new();
        for s in ALL_STATES {
            let count = agents.iter().filter(|a| a.state == s).count();
            if count > 0 {
                by_state.push(AgentStateCount { state: s, count: u32::try_from(count).unwrap_or(u32::MAX) });
            }
        }
        for a in &agents {
            usage.add(&a.usage);
            turns += u64::from(a.turns_used);
        }
        let denied = enum_str(&ToolCallStatus::Denied)?;
        let (calls, denied): (i64, i64) = self
            .db
            .call(move |c| {
                let (sql, arg) = match project {
                    Some(p) => (
                        "SELECT COUNT(*), COALESCE(SUM(t.status = ?2), 0) FROM agent_tool_calls t
                         JOIN agent_instances a ON a.id = t.agent_id WHERE a.project_id = ?1",
                        p.to_string(),
                    ),
                    None => (
                        "SELECT COUNT(*), COALESCE(SUM(t.status = ?2), 0) FROM agent_tool_calls t WHERE ?1 = ?1",
                        String::new(),
                    ),
                };
                Ok(c.query_row(sql, params![arg, denied], |r| Ok((r.get(0)?, r.get(1)?)))?)
            })
            .await?;
        Ok(AgentStats {
            total: u32::try_from(agents.len()).unwrap_or(u32::MAX),
            active,
            by_state,
            usage,
            tool_calls: u64::try_from(calls).unwrap_or(0),
            tool_calls_denied: u64::try_from(denied).unwrap_or(0),
            turns,
        })
    }
}
