//! Remote job table (Phase 6, node-protocol.md §6). **Owner: WP-B.**
//!
//! One row per job: current lease epoch, owner node, state and the stored
//! placement explanation. The task engine stays the only writer of task status;
//! job rows only track placement/lease bookkeeping.

use orkestr_storage::Migration;

/// Migrations of the job table (`job_migrations()`, appended at the end of the core list).
pub const JOB_MIGRATIONS: &[Migration] = &[Migration {
    id: "task_0601_jobs",
    owner: "task-engine",
    up: "CREATE TABLE jobs (
            id           TEXT PRIMARY KEY NOT NULL,
            task_id      TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
            project_id   TEXT NOT NULL,
            node_id      TEXT,
            epoch        INTEGER NOT NULL DEFAULT 0,
            state        TEXT NOT NULL,
            lease_until  TEXT,
            created_at   TEXT NOT NULL,
            updated_at   TEXT NOT NULL,
            doc          TEXT NOT NULL
         );
         CREATE INDEX jobs_task ON jobs(task_id);
         CREATE INDEX jobs_node_state ON jobs(node_id, state);",
    down: "DROP TABLE IF EXISTS jobs;",
}];
