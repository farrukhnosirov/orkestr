//! Docking layouts persisted per device (`ui.layouts*`, §29, §34, ADR-0013).
//!
//! Tables (owned by the core):
//! * `ui_layouts(device_id, layout_id, document, updated_at)` — one row per
//!   stored [`LayoutDocument`] (built-ins only once customised);
//! * `ui_layout_state(device_id, active_id, updated_at)` — the active preset.
//!
//! The local device id lives in `core_meta` (`device_id`) and is created on
//! first use. Rows that no longer deserialize or validate (hand-edited DB,
//! older schema) are skipped on read instead of breaking the UI — the UI then
//! falls back to the built-in preset.
//!
//! The store is stateless (wraps the shared [`Db`]); SQLite serializes writes.

use orkestr_domain::DeviceId;
use orkestr_protocol::methods::layouts::{
    is_builtin_layout, is_valid_layout_id, is_valid_layout_name, LayoutDocument, LayoutsState, MAX_LAYOUTS_PER_DEVICE,
};
use orkestr_storage::rusqlite::{params, OptionalExtension};
use orkestr_storage::{Db, Migration, StorageError, StorageResult};

/// Migrations owned by the layouts store (appended after every other list).
pub const LAYOUT_MIGRATIONS: &[Migration] = &[Migration {
    id: "core_0301_ui_layouts",
    owner: "core",
    up: "CREATE TABLE ui_layouts (
            device_id  TEXT NOT NULL,
            layout_id  TEXT NOT NULL,
            document   TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (device_id, layout_id)
         );
         CREATE TABLE ui_layout_state (
            device_id  TEXT PRIMARY KEY NOT NULL,
            active_id  TEXT NOT NULL,
            updated_at TEXT NOT NULL
         );",
    down: "DROP TABLE IF EXISTS ui_layout_state; DROP TABLE IF EXISTS ui_layouts;",
}];

/// Default active layout of a device that never switched.
pub const DEFAULT_ACTIVE_LAYOUT: &str = "coding";

/// Loads the local device id, creating it on first use.
pub async fn load_or_create_device_id(db: &Db) -> StorageResult<DeviceId> {
    db.call(|c| {
        let existing: Option<String> =
            c.query_row("SELECT value FROM core_meta WHERE key = 'device_id'", [], |r| r.get(0)).optional()?;
        if let Some(v) = existing {
            return DeviceId::parse(&v).map_err(|e| StorageError::Corrupt(e.to_string()));
        }
        let id = DeviceId::new();
        c.execute("INSERT INTO core_meta (key, value) VALUES ('device_id', ?1)", [id.to_string()])?;
        Ok(id)
    })
    .await
}

/// Per-device layout store.
#[derive(Clone)]
pub struct LayoutsStore {
    db: Db,
}

impl std::fmt::Debug for LayoutsStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LayoutsStore").finish_non_exhaustive()
    }
}

fn parse_doc(raw: &str, expected_id: &str) -> Option<LayoutDocument> {
    let doc: LayoutDocument = match serde_json::from_str(raw) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(layout = expected_id, error = %e, "skipping unreadable stored layout");
            return None;
        }
    };
    if doc.id != expected_id || doc.validate().is_err() {
        tracing::warn!(layout = expected_id, "skipping invalid stored layout");
        return None;
    }
    Some(doc)
}

impl LayoutsStore {
    /// Wraps a migrated database.
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Resolves the device: the given id (validated) or the local device.
    pub async fn device(&self, device_id: Option<&str>) -> StorageResult<DeviceId> {
        match device_id {
            Some(raw) => DeviceId::parse(raw).map_err(|e| StorageError::Invalid(e.to_string())),
            None => load_or_create_device_id(&self.db).await,
        }
    }

    /// All layouts and the active id of `device`.
    pub async fn state(&self, device: DeviceId) -> StorageResult<LayoutsState> {
        let key = device.to_string();
        let (rows, active): (Vec<(String, String)>, Option<String>) = self
            .db
            .call(move |c| {
                let mut stmt =
                    c.prepare("SELECT layout_id, document FROM ui_layouts WHERE device_id = ?1 ORDER BY layout_id")?;
                let rows = stmt
                    .query_map([&key], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                    .collect::<Result<Vec<_>, _>>()?;
                let active: Option<String> = c
                    .query_row("SELECT active_id FROM ui_layout_state WHERE device_id = ?1", [&key], |r| r.get(0))
                    .optional()?;
                Ok((rows, active))
            })
            .await?;
        let layouts: Vec<LayoutDocument> = rows.iter().filter_map(|(id, raw)| parse_doc(raw, id)).collect();
        // An active id that points at a deleted user layout falls back to the default.
        let active_id = active
            .filter(|a| is_builtin_layout(a) || layouts.iter().any(|l| &l.id == a))
            .unwrap_or_else(|| DEFAULT_ACTIVE_LAYOUT.to_owned());
        Ok(LayoutsState { device_id: device.to_string(), active_id, layouts })
    }

    /// Creates or replaces a layout (validated), optionally activating it.
    pub async fn save(&self, device: DeviceId, mut doc: LayoutDocument, activate: bool) -> StorageResult<LayoutsState> {
        doc.validate().map_err(|issues| {
            let text: Vec<String> = issues.iter().map(|i| format!("{}: {}", i.path, i.message)).collect();
            StorageError::Invalid(format!("invalid layout: {}", text.join("; ")))
        })?;
        let now = chrono::Utc::now();
        doc.updated_at = Some(now);
        let key = device.to_string();
        let id = doc.id.clone();
        let body = serde_json::to_string(&doc).map_err(StorageError::from)?;
        let ts = now.to_rfc3339();
        self.db
            .call(move |c| {
                let tx = c.transaction()?;
                let exists: bool = tx
                    .query_row(
                        "SELECT 1 FROM ui_layouts WHERE device_id = ?1 AND layout_id = ?2",
                        params![key, id],
                        |_| Ok(()),
                    )
                    .optional()?
                    .is_some();
                if !exists {
                    let count: i64 =
                        tx.query_row("SELECT COUNT(*) FROM ui_layouts WHERE device_id = ?1", [&key], |r| r.get(0))?;
                    if usize::try_from(count).unwrap_or(usize::MAX) >= MAX_LAYOUTS_PER_DEVICE {
                        return Err(StorageError::Conflict(format!(
                            "at most {MAX_LAYOUTS_PER_DEVICE} layouts per device; delete one first"
                        )));
                    }
                }
                tx.execute(
                    "INSERT INTO ui_layouts (device_id, layout_id, document, updated_at) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(device_id, layout_id) DO UPDATE SET document = excluded.document, updated_at = excluded.updated_at",
                    params![key, id, body, ts],
                )?;
                if activate {
                    set_active_tx(&tx, &key, &id, &ts)?;
                }
                tx.commit()?;
                Ok(())
            })
            .await?;
        self.state(device).await
    }

    /// Deletes a user layout or resets a built-in one (removes its stored
    /// document). Deleting the active user layout activates the default.
    pub async fn delete(&self, device: DeviceId, id: &str) -> StorageResult<LayoutsState> {
        if !is_valid_layout_id(id) {
            return Err(StorageError::Invalid("invalid layout id".into()));
        }
        let key = device.to_string();
        let id_owned = id.to_owned();
        let builtin = is_builtin_layout(id);
        let removed = self
            .db
            .call(move |c| {
                let tx = c.transaction()?;
                let n = tx.execute(
                    "DELETE FROM ui_layouts WHERE device_id = ?1 AND layout_id = ?2",
                    params![key, id_owned],
                )?;
                if !builtin {
                    tx.execute(
                        "UPDATE ui_layout_state SET active_id = ?2, updated_at = ?3 WHERE device_id = ?1 AND active_id = ?4",
                        params![key, DEFAULT_ACTIVE_LAYOUT, chrono::Utc::now().to_rfc3339(), id_owned],
                    )?;
                }
                tx.commit()?;
                Ok(n)
            })
            .await?;
        if removed == 0 && !builtin {
            return Err(StorageError::NotFound(format!("layout `{id}`")));
        }
        self.state(device).await
    }

    /// Renames a user layout.
    pub async fn rename(&self, device: DeviceId, id: &str, name: &str) -> StorageResult<LayoutsState> {
        if !is_valid_layout_name(name) {
            return Err(StorageError::Invalid("name must be 1..=80 printable characters".into()));
        }
        let state = self.state(device).await?;
        let Some(mut doc) = state.layouts.into_iter().find(|l| l.id == id) else {
            return Err(StorageError::NotFound(format!("layout `{id}`")));
        };
        doc.name = name.trim().to_owned();
        self.save(device, doc, false).await
    }

    /// Switches the active layout (built-in ids always exist).
    pub async fn set_active(&self, device: DeviceId, id: &str) -> StorageResult<LayoutsState> {
        if !is_valid_layout_id(id) {
            return Err(StorageError::Invalid("invalid layout id".into()));
        }
        let key = device.to_string();
        let id_owned = id.to_owned();
        let builtin = is_builtin_layout(id);
        self.db
            .call(move |c| {
                let tx = c.transaction()?;
                if !builtin {
                    let exists = tx
                        .query_row(
                            "SELECT 1 FROM ui_layouts WHERE device_id = ?1 AND layout_id = ?2",
                            params![key, id_owned],
                            |_| Ok(()),
                        )
                        .optional()?
                        .is_some();
                    if !exists {
                        return Err(StorageError::NotFound(format!("layout `{id_owned}`")));
                    }
                }
                set_active_tx(&tx, &key, &id_owned, &chrono::Utc::now().to_rfc3339())?;
                tx.commit()?;
                Ok(())
            })
            .await?;
        self.state(device).await
    }
}

fn set_active_tx(tx: &orkestr_storage::rusqlite::Transaction<'_>, device: &str, id: &str, ts: &str) -> StorageResult<()> {
    tx.execute(
        "INSERT INTO ui_layout_state (device_id, active_id, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(device_id) DO UPDATE SET active_id = excluded.active_id, updated_at = excluded.updated_at",
        params![device, id, ts],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use orkestr_protocol::methods::layouts::{PopoutWindowSpec, LAYOUT_SCHEMA_VERSION};

    use super::*;

    async fn store() -> (tempfile::TempDir, LayoutsStore) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path().join("l.db")).unwrap();
        db.migrate(crate::migrations::all()).await.unwrap();
        (dir, LayoutsStore::new(db))
    }

    fn doc(id: &str) -> LayoutDocument {
        LayoutDocument {
            schema_version: LAYOUT_SCHEMA_VERSION,
            id: id.into(),
            name: format!("Layout {id}"),
            based_on: None,
            dock: serde_json::json!({"grid": {"root": {}}, "panels": {"code": {"contentComponent": "code"}}}),
            panels: vec!["code".into()],
            locked: false,
            windows: Default::default(),
            updated_at: None,
        }
    }

    #[tokio::test]
    async fn device_id_is_stable() {
        let (_d, s) = store().await;
        let a = s.device(None).await.unwrap();
        let b = s.device(None).await.unwrap();
        assert_eq!(a, b);
        assert!(s.device(Some("not-a-uuid")).await.is_err());
    }

    #[tokio::test]
    async fn per_device_isolation_and_active_selection() {
        let (_d, s) = store().await;
        let a = s.device(None).await.unwrap();
        let b = DeviceId::new();
        let st = s.state(a).await.unwrap();
        assert_eq!(st.active_id, DEFAULT_ACTIVE_LAYOUT);
        assert!(st.layouts.is_empty());

        let mut d = doc("u-work");
        d.windows.popouts.push(PopoutWindowSpec { panel_id: "browser".into(), ..Default::default() });
        let st = s.save(a, d, true).await.unwrap();
        assert_eq!(st.active_id, "u-work");
        assert_eq!(st.layouts.len(), 1);
        assert!(st.layouts[0].updated_at.is_some());

        // The other device sees nothing of it.
        let other = s.state(b).await.unwrap();
        assert!(other.layouts.is_empty());
        assert_eq!(other.active_id, DEFAULT_ACTIVE_LAYOUT);
        assert!(s.set_active(b, "u-work").await.is_err(), "user layouts are per device");
        assert_eq!(s.set_active(b, "focus").await.unwrap().active_id, "focus");

        // Rename, then delete: the active id falls back to the default.
        let st = s.rename(a, "u-work", "  Deep work ").await.unwrap();
        assert_eq!(st.layouts[0].name, "Deep work");
        let st = s.delete(a, "u-work").await.unwrap();
        assert!(st.layouts.is_empty());
        assert_eq!(st.active_id, DEFAULT_ACTIVE_LAYOUT);
        assert!(s.delete(a, "u-work").await.is_err(), "deleting twice is not found");
    }

    #[tokio::test]
    async fn builtin_reset_and_validation() {
        let (_d, s) = store().await;
        let dev = s.device(None).await.unwrap();
        s.save(dev, doc("coding"), false).await.unwrap();
        assert_eq!(s.state(dev).await.unwrap().layouts.len(), 1);
        // Resetting a built-in removes the override; resetting again is fine.
        assert!(s.delete(dev, "coding").await.unwrap().layouts.is_empty());
        s.delete(dev, "coding").await.unwrap();

        let mut bad = doc("u-bad");
        bad.panels.clear();
        assert!(matches!(s.save(dev, bad, false).await, Err(StorageError::Invalid(_))));
        assert!(s.rename(dev, "missing", "x").await.is_err());
        assert!(s.set_active(dev, "Bad Id").await.is_err());
    }

    #[tokio::test]
    async fn corrupt_rows_are_skipped() {
        let (_d, s) = store().await;
        let dev = s.device(None).await.unwrap();
        s.save(dev, doc("u-ok"), false).await.unwrap();
        let key = dev.to_string();
        s.db.call(move |c| {
            c.execute(
                "INSERT INTO ui_layouts (device_id, layout_id, document, updated_at) VALUES (?1, 'u-broken', '{not json', 'x')",
                [&key],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let st = s.state(dev).await.unwrap();
        assert_eq!(st.layouts.iter().map(|l| l.id.as_str()).collect::<Vec<_>>(), vec!["u-ok"]);
    }

    #[tokio::test]
    async fn layout_count_is_bounded() {
        let (_d, s) = store().await;
        let dev = s.device(None).await.unwrap();
        for i in 0..MAX_LAYOUTS_PER_DEVICE {
            s.save(dev, doc(&format!("u-{i}")), false).await.unwrap();
        }
        assert!(matches!(s.save(dev, doc("u-extra"), false).await, Err(StorageError::Conflict(_))));
        // Replacing an existing one is still allowed.
        s.save(dev, doc("u-0"), false).await.unwrap();
    }
}
