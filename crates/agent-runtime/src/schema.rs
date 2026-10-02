//! Tables owned by the agent runtime (applied after storage/task migrations:
//! `agent_instances.project_id` references `projects`).

use orkestr_storage::Migration;

/// Migrations of this crate, in order.
pub const MIGRATIONS: &[Migration] = &[Migration {
    id: "agent_0001_runtime",
    owner: "agent-runtime",
    up: "CREATE TABLE agent_templates (
            id          TEXT PRIMARY KEY NOT NULL,
            name        TEXT NOT NULL,
            role        TEXT NOT NULL,
            source      TEXT NOT NULL,
            version     TEXT NOT NULL,
            created_at  TEXT NOT NULL,
            updated_at  TEXT NOT NULL,
            doc         TEXT NOT NULL
         );
         CREATE INDEX agent_templates_role ON agent_templates(role);
         CREATE TABLE agent_instances (
            id          TEXT PRIMARY KEY NOT NULL,
            project_id  TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            task_id     TEXT,
            template_id TEXT,
            state       TEXT NOT NULL,
            created_at  TEXT NOT NULL,
            updated_at  TEXT NOT NULL,
            doc         TEXT NOT NULL
         );
         CREATE INDEX agent_instances_project ON agent_instances(project_id, created_at);
         CREATE INDEX agent_instances_task ON agent_instances(task_id);
         CREATE INDEX agent_instances_state ON agent_instances(state);
         CREATE TABLE agent_executions (
            id          TEXT PRIMARY KEY NOT NULL,
            agent_id    TEXT NOT NULL REFERENCES agent_instances(id) ON DELETE CASCADE,
            task_id     TEXT NOT NULL,
            status      TEXT NOT NULL,
            started_at  TEXT NOT NULL,
            doc         TEXT NOT NULL
         );
         CREATE INDEX agent_executions_agent ON agent_executions(agent_id, started_at);
         CREATE TABLE agent_messages (
            seq           INTEGER PRIMARY KEY AUTOINCREMENT,
            id            TEXT NOT NULL UNIQUE,
            agent_id      TEXT NOT NULL REFERENCES agent_instances(id) ON DELETE CASCADE,
            execution_id  TEXT,
            role          TEXT NOT NULL,
            content       TEXT NOT NULL,
            tool_call_ids TEXT NOT NULL,
            created_at    TEXT NOT NULL
         );
         CREATE INDEX agent_messages_agent ON agent_messages(agent_id, seq);
         CREATE TABLE agent_tool_calls (
            seq          INTEGER PRIMARY KEY AUTOINCREMENT,
            id           TEXT NOT NULL UNIQUE,
            agent_id     TEXT NOT NULL REFERENCES agent_instances(id) ON DELETE CASCADE,
            execution_id TEXT NOT NULL,
            task_id      TEXT,
            tool         TEXT NOT NULL,
            status       TEXT NOT NULL,
            started_at   TEXT NOT NULL,
            duration_ms  INTEGER,
            result       TEXT,
            doc          TEXT NOT NULL
         );
         CREATE INDEX agent_tool_calls_agent ON agent_tool_calls(agent_id, seq);
         CREATE TABLE agent_handoffs (
            id          TEXT PRIMARY KEY NOT NULL,
            agent_id    TEXT NOT NULL REFERENCES agent_instances(id) ON DELETE CASCADE,
            task_id     TEXT NOT NULL,
            reason      TEXT NOT NULL,
            created_at  TEXT NOT NULL,
            doc         TEXT NOT NULL
         );
         CREATE INDEX agent_handoffs_agent ON agent_handoffs(agent_id, created_at);
         CREATE INDEX agent_handoffs_task ON agent_handoffs(task_id, created_at);",
    down: "DROP INDEX IF EXISTS agent_handoffs_task;
           DROP INDEX IF EXISTS agent_handoffs_agent;
           DROP TABLE IF EXISTS agent_handoffs;
           DROP INDEX IF EXISTS agent_tool_calls_agent;
           DROP TABLE IF EXISTS agent_tool_calls;
           DROP INDEX IF EXISTS agent_messages_agent;
           DROP TABLE IF EXISTS agent_messages;
           DROP INDEX IF EXISTS agent_executions_agent;
           DROP TABLE IF EXISTS agent_executions;
           DROP INDEX IF EXISTS agent_instances_state;
           DROP INDEX IF EXISTS agent_instances_task;
           DROP INDEX IF EXISTS agent_instances_project;
           DROP TABLE IF EXISTS agent_instances;
           DROP INDEX IF EXISTS agent_templates_role;
           DROP TABLE IF EXISTS agent_templates;",
}];

/// Migrations of this crate (the core appends them after the task engine's).
#[must_use]
pub fn agent_migrations() -> Vec<Migration> {
    MIGRATIONS.to_vec()
}
