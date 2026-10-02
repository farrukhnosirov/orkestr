//! Tables owned by the orchestrator (applied after the storage and agent
//! migrations: rows reference `projects`).

use orkestr_storage::Migration;

/// Migrations of this crate, in order.
pub const MIGRATIONS: &[Migration] = &[Migration {
    id: "orchestrator_0001_manager",
    owner: "orchestrator",
    up: "CREATE TABLE manager_conversations (
            id            TEXT PRIMARY KEY NOT NULL,
            project_id    TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            active        INTEGER NOT NULL,
            started_at    TEXT NOT NULL,
            ended_at      TEXT,
            message_count INTEGER NOT NULL DEFAULT 0,
            summary       TEXT
         );
         CREATE INDEX manager_conversations_project ON manager_conversations(project_id, active);
         CREATE TABLE manager_messages (
            seq             INTEGER PRIMARY KEY AUTOINCREMENT,
            id              TEXT NOT NULL UNIQUE,
            conversation_id TEXT NOT NULL REFERENCES manager_conversations(id) ON DELETE CASCADE,
            project_id      TEXT NOT NULL,
            role            TEXT NOT NULL,
            created_at      TEXT NOT NULL,
            doc             TEXT NOT NULL
         );
         CREATE INDEX manager_messages_conversation ON manager_messages(conversation_id, seq);
         CREATE TABLE manager_plans (
            id          TEXT PRIMARY KEY NOT NULL,
            project_id  TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            status      TEXT NOT NULL,
            created_at  TEXT NOT NULL,
            updated_at  TEXT NOT NULL,
            doc         TEXT NOT NULL
         );
         CREATE INDEX manager_plans_project ON manager_plans(project_id, created_at);
         CREATE TABLE manager_runs (
            id          TEXT PRIMARY KEY NOT NULL,
            project_id  TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            plan_id     TEXT NOT NULL,
            status      TEXT NOT NULL,
            started_at  TEXT NOT NULL,
            updated_at  TEXT NOT NULL,
            doc         TEXT NOT NULL
         );
         CREATE INDEX manager_runs_project ON manager_runs(project_id, started_at);
         CREATE INDEX manager_runs_status ON manager_runs(status);",
    down: "DROP INDEX IF EXISTS manager_runs_status;
           DROP INDEX IF EXISTS manager_runs_project;
           DROP TABLE IF EXISTS manager_runs;
           DROP INDEX IF EXISTS manager_plans_project;
           DROP TABLE IF EXISTS manager_plans;
           DROP INDEX IF EXISTS manager_messages_conversation;
           DROP TABLE IF EXISTS manager_messages;
           DROP INDEX IF EXISTS manager_conversations_project;
           DROP TABLE IF EXISTS manager_conversations;",
}];

/// Migrations of the orchestrator.
#[must_use]
pub fn orchestrator_migrations() -> Vec<Migration> {
    MIGRATIONS.to_vec()
}
