//! SQLite node registry (Phase 6). **Owner: WP-A.**
//!
//! `nodes` holds the coordinator's view of each remote node (a
//! [`NodeSnapshot`] document, health, last heartbeat); `node_enrollments` the
//! CIK-signed enrollment records plus the coordinator's device id on the
//! worker and coordinator-side policy overrides ([`StoredEnrollment`]). The
//! local (coordinator) node is not stored: `NodeHub::snapshots` prepends it.

use async_trait::async_trait;
use orkestr_domain::{NodeId, NodeStatus, Timestamp, TrustLevel};
use orkestr_node::registry::NodeRegistry;
use orkestr_node::NodeError;
use orkestr_protocol::methods::nodes::{NodeEnrollment, NodeHeartbeatResult, NodeSnapshot};
use orkestr_storage::rusqlite::{params, OptionalExtension};
use orkestr_storage::{Db, Migration, StorageError};
use serde::{Deserialize, Serialize};

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

/// Enrollment as stored on the coordinator (`node_enrollments.doc`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredEnrollment {
    /// The signed enrollment.
    pub enrollment: NodeEnrollment,
    /// Device id the worker assigned to this coordinator (E2E handshake identity).
    pub device_id: String,
    /// Coordinator-side slot limit (`nodes.setPolicy`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent_jobs: Option<u32>,
}

fn storage(e: StorageError) -> NodeError {
    NodeError::Internal(e.to_string())
}

fn wire<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default()
}

fn is_sticky(s: NodeStatus) -> bool {
    matches!(s, NodeStatus::Draining | NodeStatus::Revoked)
}

/// SQLite-backed [`NodeRegistry`].
#[derive(Debug, Clone)]
pub struct SqlNodeRegistry {
    db: Db,
}

impl SqlNodeRegistry {
    /// Registry over the core database (migrations applied by the core).
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Stored enrollment incl. device id and overrides.
    ///
    /// # Errors
    /// Storage failure.
    pub async fn stored(&self, id: NodeId) -> Result<Option<StoredEnrollment>, NodeError> {
        let key = id.to_string();
        self.db
            .call(move |c| {
                let doc: Option<String> = c
                    .query_row("SELECT doc FROM node_enrollments WHERE node_id=?1", params![key], |r| r.get(0))
                    .optional()?;
                Ok(match doc {
                    Some(d) => Some(serde_json::from_str(&d)?),
                    None => None,
                })
            })
            .await
            .map_err(storage)
    }

    /// Stores / replaces an enrollment with its device id and overrides.
    ///
    /// # Errors
    /// Storage failure.
    pub async fn put_stored(&self, s: StoredEnrollment) -> Result<(), NodeError> {
        self.db
            .call(move |c| {
                let e = &s.enrollment;
                c.execute(
                    "INSERT INTO node_enrollments(node_id, core_id, trust_level, enrolled_at, revoked_at, doc)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT(node_id) DO UPDATE SET core_id=excluded.core_id, trust_level=excluded.trust_level,
                        enrolled_at=excluded.enrolled_at, revoked_at=excluded.revoked_at, doc=excluded.doc",
                    params![
                        e.node_id.to_string(),
                        e.core_id,
                        wire(&e.trust_level),
                        e.enrolled_at.to_rfc3339(),
                        e.revoked_at.map(|t| t.to_rfc3339()),
                        serde_json::to_string(&s)?,
                    ],
                )?;
                Ok(())
            })
            .await
            .map_err(storage)
    }

    /// All stored enrollments.
    ///
    /// # Errors
    /// Storage failure.
    pub async fn all_stored(&self) -> Result<Vec<StoredEnrollment>, NodeError> {
        self.db
            .call(|c| {
                let mut st = c.prepare("SELECT doc FROM node_enrollments ORDER BY enrolled_at, node_id")?;
                let docs = st.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
                docs.iter().map(|d| serde_json::from_str(d).map_err(StorageError::from)).collect()
            })
            .await
            .map_err(storage)
    }

    /// Stores / replaces a node snapshot.
    ///
    /// # Errors
    /// Storage failure.
    pub async fn put_snapshot(&self, snap: NodeSnapshot) -> Result<(), NodeError> {
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO nodes(id, status, last_heartbeat_at, updated_at, doc) VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(id) DO UPDATE SET status=excluded.status,
                        last_heartbeat_at=excluded.last_heartbeat_at, updated_at=excluded.updated_at, doc=excluded.doc",
                    params![
                        snap.node.id.to_string(),
                        wire(&snap.status),
                        snap.last_heartbeat_at.map(|t| t.to_rfc3339()),
                        chrono::Utc::now().to_rfc3339(),
                        serde_json::to_string(&snap)?,
                    ],
                )?;
                Ok(())
            })
            .await
            .map_err(storage)
    }

    /// Read-modify-write of one snapshot.
    ///
    /// # Errors
    /// Unknown node, storage failure.
    pub async fn update_snapshot(
        &self,
        id: NodeId,
        f: impl FnOnce(&mut NodeSnapshot) + Send,
    ) -> Result<NodeSnapshot, NodeError> {
        let mut snap = self.get(id).await?.ok_or(NodeError::NotFound(id))?;
        f(&mut snap);
        snap.node.status = snap.status;
        self.put_snapshot(snap.clone()).await?;
        Ok(snap)
    }
}

#[async_trait]
impl NodeRegistry for SqlNodeRegistry {
    async fn list(&self) -> Result<Vec<NodeSnapshot>, NodeError> {
        self.db
            .call(|c| {
                let mut st = c.prepare("SELECT doc FROM nodes ORDER BY id")?;
                let docs = st.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
                docs.iter().map(|d| serde_json::from_str(d).map_err(StorageError::from)).collect()
            })
            .await
            .map_err(storage)
    }

    async fn get(&self, id: NodeId) -> Result<Option<NodeSnapshot>, NodeError> {
        let key = id.to_string();
        self.db
            .call(move |c| {
                let doc: Option<String> =
                    c.query_row("SELECT doc FROM nodes WHERE id=?1", params![key], |r| r.get(0)).optional()?;
                Ok(match doc {
                    Some(d) => Some(serde_json::from_str(&d)?),
                    None => None,
                })
            })
            .await
            .map_err(storage)
    }

    async fn enrollment(&self, id: NodeId) -> Result<Option<NodeEnrollment>, NodeError> {
        Ok(self.stored(id).await?.map(|s| s.enrollment))
    }

    async fn enrollments(&self) -> Result<Vec<NodeEnrollment>, NodeError> {
        Ok(self.all_stored().await?.into_iter().map(|s| s.enrollment).collect())
    }

    async fn upsert_enrollment(&self, enrollment: NodeEnrollment) -> Result<(), NodeError> {
        let id = enrollment.node_id;
        let stored = match self.stored(id).await? {
            Some(s) => StoredEnrollment { enrollment, ..s },
            None => StoredEnrollment { enrollment, device_id: String::new(), max_concurrent_jobs: None },
        };
        self.put_stored(stored).await
    }

    async fn record_heartbeat(
        &self,
        id: NodeId,
        hb: NodeHeartbeatResult,
        at: Timestamp,
    ) -> Result<NodeSnapshot, NodeError> {
        let max_override = self.stored(id).await?.and_then(|s| s.max_concurrent_jobs);
        self.update_snapshot(id, move |s| {
            let mut node = hb.node;
            node.is_local = false;
            node.last_heartbeat_at = Some(at);
            s.node = node;
            s.slots = hb.slots;
            if let Some(max) = max_override {
                s.slots.max = s.slots.max.min(max);
            }
            s.leases = hb.leases;
            s.model_digests = hb.model_digests;
            s.last_heartbeat_at = Some(at);
            if !is_sticky(s.status) {
                s.status = NodeStatus::Online;
            }
        })
        .await
    }

    async fn set_status(&self, id: NodeId, status: NodeStatus) -> Result<NodeSnapshot, NodeError> {
        self.update_snapshot(id, move |s| s.status = status).await
    }
}

/// Effective trust: never above what the worker granted.
#[must_use]
pub fn effective_trust(granted: TrustLevel, requested: Option<TrustLevel>) -> TrustLevel {
    requested.map_or(granted, |r| r.min(granted))
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_domain::{Arch, Node, NodeResources, OsKind};
    use orkestr_protocol::methods::nodes::NodeSlots;

    pub(crate) fn node(id: NodeId) -> Node {
        Node {
            id,
            name: "gpu".into(),
            hostname: "gpu".into(),
            os: OsKind::Linux,
            os_version: "6".into(),
            arch: Arch::X64,
            core_version: "0.1.0".into(),
            status: NodeStatus::Online,
            resources: NodeResources {
                cpu_cores: 8,
                cpu_usage_percent: 1.0,
                memory_total_bytes: 1 << 34,
                memory_used_bytes: 1 << 30,
                disk_total_bytes: 1 << 40,
                disk_free_bytes: 1 << 39,
                gpus: Vec::new(),
            },
            tools: vec![],
            local_models: vec![],
            is_local: false,
            last_heartbeat_at: None,
        }
    }

    async fn registry() -> SqlNodeRegistry {
        let db = Db::open_in_memory().unwrap();
        db.migrate(NODE_MIGRATIONS.to_vec()).await.unwrap();
        SqlNodeRegistry::new(db)
    }

    #[tokio::test]
    async fn enrollment_snapshot_heartbeat_round_trip() {
        let r = registry().await;
        let id = NodeId::new();
        let enr = NodeEnrollment {
            node_id: id,
            core_id: "c2".into(),
            name: "gpu".into(),
            public_key: "pk".into(),
            trust_level: TrustLevel::Standard,
            allowed_projects: vec!["*".into()],
            enrolled_at: chrono::Utc::now(),
            revoked_at: None,
            sig: "s".into(),
        };
        r.put_stored(StoredEnrollment { enrollment: enr.clone(), device_id: "d1".into(), max_concurrent_jobs: Some(1) })
            .await
            .unwrap();
        assert_eq!(r.enrollment(id).await.unwrap(), Some(enr.clone()));
        assert_eq!(r.stored(id).await.unwrap().unwrap().device_id, "d1");
        let snap = NodeSnapshot {
            node: node(id),
            status: NodeStatus::Online,
            trust_level: TrustLevel::Standard,
            allowed_projects: vec!["*".into()],
            slots: NodeSlots { max: 4, used: 0 },
            leases: vec![],
            last_heartbeat_at: None,
            is_coordinator: false,
            labels: Default::default(),
            model_digests: vec![],
        };
        r.put_snapshot(snap).await.unwrap();
        r.set_status(id, NodeStatus::Suspect).await.unwrap();
        let hb = NodeHeartbeatResult {
            node: node(id),
            sample: None,
            slots: NodeSlots { max: 4, used: 1 },
            leases: vec![],
            model_digests: vec![],
        };
        let at = chrono::Utc::now();
        let s = r.record_heartbeat(id, hb.clone(), at).await.unwrap();
        assert_eq!(s.status, NodeStatus::Online, "a heartbeat brings a suspect node back");
        assert_eq!(s.slots, NodeSlots { max: 1, used: 1 }, "coordinator slot override");
        assert_eq!(s.last_heartbeat_at, Some(at));
        r.set_status(id, NodeStatus::Draining).await.unwrap();
        let s = r.record_heartbeat(id, hb, at).await.unwrap();
        assert_eq!(s.status, NodeStatus::Draining, "draining is sticky");
        assert_eq!(r.list().await.unwrap().len(), 1);
        assert!(r.get(NodeId::new()).await.unwrap().is_none());
        assert!(matches!(r.set_status(NodeId::new(), NodeStatus::Online).await, Err(NodeError::NotFound(_))));
        assert_eq!(effective_trust(TrustLevel::Standard, Some(TrustLevel::Trusted)), TrustLevel::Standard);
        assert_eq!(effective_trust(TrustLevel::Trusted, Some(TrustLevel::Restricted)), TrustLevel::Restricted);
    }
}
