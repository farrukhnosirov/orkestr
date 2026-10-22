//! Composition of all engine migrations in a **fixed order** (contract §3).
//!
//! Append new engine migration lists at the end. Never reorder; never edit an
//! applied migration (checksums are verified).

use orkestr_domain::NodeId;
use orkestr_storage::rusqlite::OptionalExtension;
use orkestr_storage::{Db, Migration, StorageError, StorageResult};

/// Migrations owned by the core itself.
pub const CORE_MIGRATIONS: &[Migration] = &[Migration {
    id: "core_0001_meta",
    owner: "core",
    up: "CREATE TABLE core_meta (key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL);",
    down: "DROP TABLE IF EXISTS core_meta;",
}];

/// All migrations in application order.
#[must_use]
pub fn all() -> Vec<Migration> {
    let mut all = Vec::new();
    all.extend_from_slice(CORE_MIGRATIONS);
    all.extend(orkestr_storage::storage_migrations());
    all.extend(orkestr_events::events_migrations());
    all.extend(orkestr_security::migrations());
    all.extend(orkestr_providers::migrations());
    all.extend(orkestr_task_engine::task_migrations());
    all.extend(orkestr_vcs::vcs_migrations());
    all.extend(orkestr_snapshots::snapshot_migrations());
    all.extend(orkestr_terminal::terminal_migrations());
    all.extend(orkestr_files::files_migrations());
    // Phase 1+: task-engine, agent-runtime, providers, memory, cost, … append here.
    all.extend(orkestr_memory::memory_migrations());
    all.extend(orkestr_cost::cost_migrations());
    all.extend_from_slice(crate::settings_store::SETTINGS_MIGRATIONS);
    all.extend(orkestr_agent_runtime::agent_migrations());
    all.extend(orkestr_orchestrator::orchestrator_migrations());
    all.extend(orkestr_quality::quality_migrations());
    // Phase 2 (review-consensus)
    all.extend(orkestr_orchestrator::review_migrations());
    all.extend(orkestr_insight::insight_migrations());
    all.extend(orkestr_orchestrator::merge_migrations());
    // Phase 2 (quality pipeline + self-healing)
    all.extend(orkestr_orchestrator::pipeline_migrations());
    all.extend_from_slice(crate::ui_store::UI_MIGRATIONS);
    // layout-multimonitor (Phase 3): per-device docking layouts
    all.extend_from_slice(crate::layouts_store::LAYOUT_MIGRATIONS);
    all.extend_from_slice(crate::rpc::robots::ROBOTS_MIGRATIONS);
    // VS Code extensions + language-server trust (ADR-0025)
    all.extend(orkestr_extensions::extensions_migrations());
    // multi-node (Phase 6): node registry + enrollments (WP-A), remote jobs (WP-B)
    all.extend_from_slice(crate::nodes::registry_store::NODE_MIGRATIONS);
    all.extend(orkestr_task_engine::job_migrations());
    all
}

/// Loads the persistent node id, creating it on first start.
pub async fn load_or_create_node_id(db: &Db) -> StorageResult<NodeId> {
    db.call(|c| {
        let existing: Option<String> =
            c.query_row("SELECT value FROM core_meta WHERE key = 'node_id'", [], |r| r.get(0)).optional()?;
        if let Some(v) = existing {
            return NodeId::parse(&v).map_err(|e| StorageError::Corrupt(e.to_string()));
        }
        let id = NodeId::new();
        c.execute("INSERT INTO core_meta (key, value) VALUES ('node_id', ?1)", [id.to_string()])?;
        Ok(id)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn all_migrations_apply_and_fully_revert() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path().join("m.db")).unwrap();
        let report = db.migrate(all()).await.unwrap();
        assert_eq!(report.applied.len(), all().len());
        let a = load_or_create_node_id(&db).await.unwrap();
        let b = load_or_create_node_id(&db).await.unwrap();
        assert_eq!(a, b, "node id must be stable");
        let n = all().len();
        let reverted = db.call(move |c| orkestr_storage::Migrator::rollback_last(c, n)).await.unwrap();
        assert_eq!(reverted.len(), n);
        let tables: i64 = db
            .call(|c| {
                Ok(c.query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' AND name != '_orkestr_migrations'",
                    [],
                    |r| r.get(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(tables, 0, "down migrations must remove every table");
    }
}
