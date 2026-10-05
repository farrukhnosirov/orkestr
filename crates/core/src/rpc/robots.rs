//! `robots.*` handlers (§30, ADR-0016): the robots document (designs,
//! positions, visibility limits) and the list of agents with their resolved
//! robot design.
//!
//! The document is one validated JSON row in `robots_config`. Patches are
//! serialized by a process-wide lock (robot windows save positions
//! concurrently), validated and only then persisted. A stored document that
//! no longer validates (hand-edited DB, newer build) is ignored in favour of
//! the defaults instead of breaking the robots.

use std::collections::HashMap;

use orkestr_domain::{AgentTemplateId, AvatarConfig};
use orkestr_protocol::methods::agents::ListAgentsParams;
use orkestr_protocol::methods::robots::{
    self as m, ListRobotsParams, ListRobotsResult, RobotEntry, RobotsConfig, RobotsConfigResult, SetRobotsConfigParams,
};
use orkestr_protocol::methods::Empty;
use orkestr_protocol::RpcError;
use orkestr_storage::rusqlite::OptionalExtension;
use orkestr_storage::{Db, Migration, StorageError, StorageResult};

use super::agents::agent_to_rpc;
use super::{RpcContext, RpcRouter};
use crate::error::storage_to_rpc;

/// Migrations owned by the robots area (appended in `migrations::all`).
pub const ROBOTS_MIGRATIONS: &[Migration] = &[Migration {
    id: "robots_0001_config",
    owner: "core-robots",
    up: "CREATE TABLE robots_config (
            id         INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
            doc        TEXT NOT NULL,
            updated_at TEXT NOT NULL
         );",
    down: "DROP TABLE IF EXISTS robots_config;",
}];

/// Serializes read-modify-write cycles of the document.
static WRITE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::List, _, _>(list)
        .method::<m::Config, _, _>(|ctx: RpcContext, _: Empty| async move {
            Ok(RobotsConfigResult { config: load(&ctx.services.db).await.map_err(storage_to_rpc)? })
        })
        .method::<m::SetConfig, _, _>(|ctx: RpcContext, p: SetRobotsConfigParams| async move {
            Ok(RobotsConfigResult { config: set_config(&ctx.services.db, p).await? })
        });
}

/// Loads the document (defaults when absent or invalid).
pub async fn load(db: &Db) -> StorageResult<RobotsConfig> {
    let raw: Option<String> = db
        .call(|c| Ok(c.query_row("SELECT doc FROM robots_config WHERE id = 1", [], |r| r.get(0)).optional()?))
        .await?;
    Ok(parse_stored(raw.as_deref()))
}

/// Parses a stored document, falling back to the defaults.
#[must_use]
pub fn parse_stored(raw: Option<&str>) -> RobotsConfig {
    let Some(raw) = raw else { return RobotsConfig::default() };
    match serde_json::from_str::<RobotsConfig>(raw) {
        Ok(cfg) if cfg.validate().is_ok() => cfg,
        Ok(_) => {
            tracing::warn!("stored robots config does not validate; using defaults");
            RobotsConfig::default()
        }
        Err(e) => {
            tracing::warn!(error = %e, "stored robots config is unreadable; using defaults");
            RobotsConfig::default()
        }
    }
}

async fn save(db: &Db, cfg: &RobotsConfig) -> StorageResult<()> {
    let doc = serde_json::to_string(cfg).map_err(StorageError::from)?;
    let now = chrono::Utc::now().to_rfc3339();
    db.call(move |c| {
        c.execute(
            "INSERT INTO robots_config (id, doc, updated_at) VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET doc = excluded.doc, updated_at = excluded.updated_at",
            orkestr_storage::rusqlite::params![doc, now],
        )?;
        Ok(())
    })
    .await
}

/// Applies a validated patch atomically and returns the new document.
pub async fn set_config(db: &Db, patch: SetRobotsConfigParams) -> Result<RobotsConfig, RpcError> {
    let _guard = WRITE_LOCK.lock().await;
    let current = load(db).await.map_err(storage_to_rpc)?;
    let next = patch.apply(current);
    if let Err(issues) = next.validate() {
        let data = serde_json::to_value(&issues).unwrap_or(serde_json::Value::Null);
        let first = issues.first().map(|i| format!("{}: {}", i.path, i.message)).unwrap_or_default();
        return Err(RpcError::bad_request(format!("invalid robots config: {first}")).with_data(data));
    }
    save(db, &next).await.map_err(storage_to_rpc)?;
    Ok(next)
}

async fn list(ctx: RpcContext, p: ListRobotsParams) -> Result<ListRobotsResult, RpcError> {
    let services = &ctx.services;
    let config = load(&services.db).await.map_err(storage_to_rpc)?;
    let include_finished = p.include_finished.unwrap_or(false);
    let agents = services
        .agents
        .list(ListAgentsParams {
            project_id: p.project_id,
            task_id: None,
            active_only: if include_finished { None } else { Some(true) },
        })
        .await
        .map_err(agent_to_rpc)?;
    let wanted = p.agent_ids.as_ref();
    let mut avatars: HashMap<AgentTemplateId, Option<AvatarConfig>> = HashMap::new();
    let mut robots = Vec::new();
    for agent in agents {
        if wanted.is_some_and(|ids| !ids.contains(&agent.id)) {
            continue;
        }
        let avatar = match agent.template_id {
            Some(tid) => {
                if let Some(cached) = avatars.get(&tid) {
                    cached.clone()
                } else {
                    // A deleted template keeps its agents; they get the default design.
                    let a = services.agents.templates().get(tid).await.ok().and_then(|t| t.avatar);
                    avatars.insert(tid, a.clone());
                    a
                }
            }
            None => None,
        };
        let tid = agent.template_id.map(|t| t.to_string());
        let (design, floating) = config.resolve(tid.as_deref(), avatar.as_ref());
        robots.push(RobotEntry { agent, design, floating });
    }
    Ok(ListRobotsResult { robots })
}

#[cfg(test)]
mod tests {
    use orkestr_protocol::methods::robots::{RobotDesign, RobotDesignUpsert, RobotPosition, RobotPositionUpsert};

    use super::*;

    async fn db() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path().join("r.db")).unwrap();
        db.migrate(crate::migrations::all()).await.unwrap();
        (dir, db)
    }

    #[tokio::test]
    async fn defaults_then_patches_persist() {
        let (_dir, db) = db().await;
        assert_eq!(load(&db).await.unwrap(), RobotsConfig::default());
        let design = RobotDesign { skin: "ember".into(), ..RobotDesign::default() };
        let cfg = set_config(
            &db,
            SetRobotsConfigParams {
                enabled: Some(false),
                design: Some(RobotDesignUpsert { template_id: "tpl-1".into(), design: Some(design.clone()) }),
                ..SetRobotsConfigParams::default()
            },
        )
        .await
        .unwrap();
        assert!(!cfg.enabled);
        assert_eq!(load(&db).await.unwrap(), cfg);
        // Concurrent position saves from several robot windows are all kept.
        let mut joins = Vec::new();
        for i in 0..8 {
            let db = db.clone();
            joins.push(tokio::spawn(async move {
                set_config(
                    &db,
                    SetRobotsConfigParams {
                        position: Some(RobotPositionUpsert {
                            key: format!("tpl-{i}"),
                            position: Some(RobotPosition { monitor: None, x: f64::from(i), y: 1.0 }),
                        }),
                        ..SetRobotsConfigParams::default()
                    },
                )
                .await
                .unwrap();
            }));
        }
        for j in joins {
            j.await.unwrap();
        }
        let cfg = load(&db).await.unwrap();
        assert_eq!(cfg.positions.len(), 8);
        assert_eq!(cfg.designs.get("tpl-1"), Some(&design));
    }

    #[tokio::test]
    async fn invalid_patch_is_rejected_and_not_saved() {
        let (_dir, db) = db().await;
        let err = set_config(&db, SetRobotsConfigParams { max_visible: Some(0), ..SetRobotsConfigParams::default() })
            .await
            .unwrap_err();
        assert_eq!(err.code.as_str(), "bad_request");
        assert!(err.message.contains("maxVisible"), "{}", err.message);
        assert_eq!(load(&db).await.unwrap(), RobotsConfig::default());
    }

    #[test]
    fn unreadable_or_invalid_documents_fall_back_to_defaults() {
        assert_eq!(parse_stored(Some("not json")), RobotsConfig::default());
        let bad = RobotsConfig { max_visible: 99, ..RobotsConfig::default() };
        assert_eq!(parse_stored(Some(&serde_json::to_string(&bad).unwrap())), RobotsConfig::default());
        let good = RobotsConfig { max_visible: 2, ..RobotsConfig::default() };
        assert_eq!(parse_stored(Some(&serde_json::to_string(&good).unwrap())), good);
    }
}
