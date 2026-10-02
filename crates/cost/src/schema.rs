//! Tables owned by the cost engine (data-model §4.25, budgets/pricing section).
//!
//! `usage_records` is append-only analytics data: it intentionally has no
//! foreign keys so history survives project/task deletion. Day/month keys are
//! computed at ingest time with the engine's calendar.

use orkestr_storage::Migration;

/// Usage records.
pub const USAGE: Migration = Migration {
    id: "cost_0001_usage_records",
    owner: "cost",
    up: "CREATE TABLE usage_records (
            seq                 INTEGER PRIMARY KEY AUTOINCREMENT,
            id                  TEXT NOT NULL UNIQUE,
            ts                  TEXT NOT NULL,
            day                 TEXT NOT NULL,
            month               TEXT NOT NULL,
            provider_id         TEXT NOT NULL,
            model               TEXT NOT NULL,
            project_id          TEXT,
            task_id             TEXT,
            agent_id            TEXT,
            execution_id        TEXT,
            input_tokens        INTEGER NOT NULL,
            output_tokens       INTEGER NOT NULL,
            cached_input_tokens INTEGER NOT NULL,
            reasoning_tokens    INTEGER NOT NULL,
            cost_micros         INTEGER NOT NULL,
            estimated           INTEGER NOT NULL,
            latency_ms          INTEGER,
            success             INTEGER NOT NULL
         );
         CREATE INDEX usage_records_ts ON usage_records(ts);
         CREATE INDEX usage_records_day ON usage_records(day);
         CREATE INDEX usage_records_project ON usage_records(project_id, day);
         CREATE INDEX usage_records_task ON usage_records(task_id);",
    down: "DROP INDEX IF EXISTS usage_records_task;
           DROP INDEX IF EXISTS usage_records_project;
           DROP INDEX IF EXISTS usage_records_day;
           DROP INDEX IF EXISTS usage_records_ts;
           DROP TABLE IF EXISTS usage_records;",
};

/// Budgets and fired threshold alerts.
pub const BUDGETS: Migration = Migration {
    id: "cost_0002_budgets",
    owner: "cost",
    up: "CREATE TABLE budgets (
            id         TEXT PRIMARY KEY NOT NULL,
            scope      TEXT NOT NULL,
            scope_id   TEXT,
            period     TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            doc        TEXT NOT NULL
         );
         CREATE UNIQUE INDEX budgets_scope_period ON budgets(scope, IFNULL(scope_id, ''), period);
         CREATE TABLE budget_alerts (
            budget_id  TEXT NOT NULL REFERENCES budgets(id) ON DELETE CASCADE,
            period_key TEXT NOT NULL,
            level_pct  INTEGER NOT NULL,
            fired_at   TEXT NOT NULL,
            PRIMARY KEY (budget_id, period_key, level_pct)
         );",
    down: "DROP TABLE IF EXISTS budget_alerts;
           DROP INDEX IF EXISTS budgets_scope_period;
           DROP TABLE IF EXISTS budgets;",
};

/// All cost migrations in order.
#[must_use]
pub fn cost_migrations() -> Vec<orkestr_storage::Migration> {
    vec![USAGE, BUDGETS]
}
