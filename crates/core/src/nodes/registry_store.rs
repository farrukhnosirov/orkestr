//! SQLite node registry (Phase 6). **Owner: WP-A.**
//!
//! `nodes` holds the coordinator's view of each node (descriptor JSON, health,
//! last heartbeat); `node_enrollments` the signed enrollment records.

use orkestr_storage::Migration;

/// Node registry migrations (appended at the end of `migrations::all()`).
pub const NODE_MIGRATIONS: &[Migration] = &[Migration {
    id: "core_0601_nodes",
    owner: "core",
    up: "CREATE TABLE node_enrollments (
            node_id      TEXT PRIMARY KEY NOT NULL,
            core_id      TEXT NOT NULL,
            trust_level  TEXT NOT NULL,
            enrolled_at  TEXT NOT NULL,
            revoked_at   TEXT,
            doc          TEXT NOT NULL
         );
         CREATE TABLE nodes (
            id                 TEXT PRIMARY KEY NOT NULL,
            status             TEXT NOT NULL,
            last_heartbeat_at  TEXT,
            updated_at         TEXT NOT NULL,
            doc                TEXT NOT NULL
         );",
    down: "DROP TABLE IF EXISTS nodes; DROP TABLE IF EXISTS node_enrollments;",
}];

/// SQLite-backed `orkestr_node::NodeRegistry` (Step 0 skeleton; WP-A implements).
#[derive(Debug, Default)]
pub struct SqlNodeRegistry;
