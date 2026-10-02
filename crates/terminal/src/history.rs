//! Command history (`terminal_commands`, ADR-0014): every gated command —
//! agent `run`s, user `run`s and lines typed into agent PTYs — is recorded
//! with its gate decision and outcome. The command text is redacted before
//! it is stored (contract §6: no secrets in SQLite plaintext).

use async_trait::async_trait;
use orkestr_domain::{AgentId, ProjectId, TaskId, TerminalId, Timestamp};
use orkestr_storage::{Db, Migration, StorageResult};

/// Migrations owned by this crate (append-only).
pub const TERMINAL_MIGRATIONS: &[Migration] = &[Migration {
    id: "terminal_0001_commands",
    owner: "terminal",
    up: "CREATE TABLE terminal_commands (
        id TEXT PRIMARY KEY NOT NULL,
        terminal_id TEXT,
        project_id TEXT,
        task_id TEXT,
        agent_id TEXT,
        mode TEXT NOT NULL,
        shell TEXT NOT NULL,
        command_redacted TEXT NOT NULL,
        decision TEXT NOT NULL,
        exit_code INTEGER,
        timed_out INTEGER NOT NULL DEFAULT 0,
        started_at TEXT NOT NULL,
        finished_at TEXT
    );
    CREATE INDEX terminal_commands_project ON terminal_commands(project_id, started_at);
    CREATE INDEX terminal_commands_agent ON terminal_commands(agent_id, started_at);",
    down: "DROP INDEX IF EXISTS terminal_commands_agent;
    DROP INDEX IF EXISTS terminal_commands_project;
    DROP TABLE IF EXISTS terminal_commands;",
}];

/// Migrations of the terminal crate, in order.
#[must_use]
pub fn terminal_migrations() -> Vec<Migration> {
    TERMINAL_MIGRATIONS.to_vec()
}

/// One history row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRecord {
    /// Session (PTY or view).
    pub terminal_id: Option<TerminalId>,
    /// Project.
    pub project_id: Option<ProjectId>,
    /// Task.
    pub task_id: Option<TaskId>,
    /// Agent (None = user).
    pub agent_id: Option<AgentId>,
    /// `run` or `line`.
    pub mode: &'static str,
    /// Shell id.
    pub shell: String,
    /// Raw command (redacted by the log before storing).
    pub command: String,
    /// Gate decision label (`allow`, `deny`, `ask`).
    pub decision: &'static str,
    /// Exit code.
    pub exit_code: Option<i32>,
    /// Killed by timeout.
    pub timed_out: bool,
    /// Start.
    pub started_at: Timestamp,
    /// End.
    pub finished_at: Option<Timestamp>,
}

/// Stores command history.
#[async_trait]
pub trait CommandLog: Send + Sync + 'static {
    /// Records a command (best effort; failures are logged by the caller).
    async fn record(&self, record: CommandRecord) -> Result<(), String>;
}

/// Discards history.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopCommandLog;

#[async_trait]
impl CommandLog for NoopCommandLog {
    async fn record(&self, _record: CommandRecord) -> Result<(), String> {
        Ok(())
    }
}

/// SQLite-backed history.
#[derive(Clone)]
pub struct SqliteCommandLog {
    db: Db,
    redact: fn(&str) -> String,
}

impl std::fmt::Debug for SqliteCommandLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteCommandLog").finish_non_exhaustive()
    }
}

/// A stored history row (as read back).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredCommand {
    /// Row id.
    pub id: String,
    /// Agent.
    pub agent_id: Option<String>,
    /// Mode.
    pub mode: String,
    /// Redacted command.
    pub command: String,
    /// Decision.
    pub decision: String,
    /// Exit code.
    pub exit_code: Option<i32>,
    /// Timed out.
    pub timed_out: bool,
}

impl SqliteCommandLog {
    /// New log over `db` (migrations must be applied). `redact` scrubs
    /// secrets from command text before storing it.
    #[must_use]
    pub fn new(db: Db, redact: fn(&str) -> String) -> Self {
        Self { db, redact }
    }

    /// Most recent commands of a project (newest first).
    pub async fn recent(&self, project_id: Option<ProjectId>, limit: usize) -> StorageResult<Vec<StoredCommand>> {
        let limit = i64::try_from(limit.min(1000)).unwrap_or(1000);
        self.db
            .call(move |c| {
                let mut stmt = c.prepare(
                    "SELECT id, agent_id, mode, command_redacted, decision, exit_code, timed_out
                     FROM terminal_commands
                     WHERE (?1 IS NULL AND project_id IS NULL) OR project_id = ?1
                     ORDER BY started_at DESC, id DESC LIMIT ?2",
                )?;
                let rows = stmt
                    .query_map(orkestr_storage::rusqlite::params![project_id.map(|p| p.to_string()), limit], |r| {
                        Ok(StoredCommand {
                            id: r.get(0)?,
                            agent_id: r.get(1)?,
                            mode: r.get(2)?,
                            command: r.get(3)?,
                            decision: r.get(4)?,
                            exit_code: r.get(5)?,
                            timed_out: r.get::<_, i64>(6)? != 0,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await
    }
}

#[async_trait]
impl CommandLog for SqliteCommandLog {
    async fn record(&self, r: CommandRecord) -> Result<(), String> {
        let command = (self.redact)(&r.command);
        let id = uuid_v7();
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO terminal_commands (id, terminal_id, project_id, task_id, agent_id, mode, shell,
                        command_redacted, decision, exit_code, timed_out, started_at, finished_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                    orkestr_storage::rusqlite::params![
                        id,
                        r.terminal_id.map(|t| t.to_string()),
                        r.project_id.map(|p| p.to_string()),
                        r.task_id.map(|t| t.to_string()),
                        r.agent_id.map(|a| a.to_string()),
                        r.mode,
                        r.shell,
                        command,
                        r.decision,
                        r.exit_code,
                        i64::from(r.timed_out),
                        r.started_at.to_rfc3339(),
                        r.finished_at.map(|t| t.to_rfc3339()),
                    ],
                )?;
                Ok(())
            })
            .await
            .map_err(|e| e.to_string())
    }
}

fn uuid_v7() -> String {
    // Reuse the domain id generator (UUID v7) for time-ordered row ids.
    TerminalId::new().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn redact(s: &str) -> String {
        s.replace("hunter2", "[REDACTED]")
    }

    #[tokio::test]
    async fn records_and_reads_back_redacted() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path().join("t.db")).unwrap();
        db.migrate(terminal_migrations()).await.unwrap();
        let log = SqliteCommandLog::new(db, redact);
        let pid = ProjectId::new();
        log.record(CommandRecord {
            terminal_id: None,
            project_id: Some(pid),
            task_id: None,
            agent_id: Some(AgentId::new()),
            mode: "run",
            shell: "bash".into(),
            command: "mysql -phunter2".into(),
            decision: "allow",
            exit_code: Some(0),
            timed_out: false,
            started_at: orkestr_domain::now(),
            finished_at: Some(orkestr_domain::now()),
        })
        .await
        .unwrap();
        let rows = log.recent(Some(pid), 10).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].command, "mysql -p[REDACTED]");
        assert_eq!(rows[0].exit_code, Some(0));
        assert!(log.recent(None, 10).await.unwrap().is_empty());
    }
}
