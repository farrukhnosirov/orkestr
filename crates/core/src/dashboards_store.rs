//! Dashboard Builder persistence and validation (Phase 7, ADR-0034). **Owner: WP3.**
//!
//! Tables are prefixed `dashboards_`; [`DASHBOARD_MIGRATIONS`] is composed by
//! [`crate::migrations::all`].
//!
//! * `dashboards_dashboards(id, project_id, name, widgets, version, created_at, updated_at)`
//!   — one row per dashboard; `project_id` NULL = global; `widgets` is the JSON
//!   array of [`WidgetInstance`] (12-column grid placements + per-instance config).
//!
//! Validation ([`validate_layout`]) is pure: the RPC layer resolves the widget
//! catalogue (built-ins + enabled plugin widgets) and passes it in. Per-instance
//! config is checked against the widget's `configSchema` with a small,
//! fail-closed JSON-schema subset ([`config_schema`]).

use std::collections::{HashMap, HashSet};

use orkestr_domain::{DashboardId, ProjectId, Timestamp};
use orkestr_protocol::methods::dashboards::{
    Dashboard, DashboardSummary, WidgetInstance, DASHBOARD_COLUMNS, MAX_DASHBOARD_WIDGETS, MAX_WIDGET_CONFIG_BYTES,
};
use orkestr_protocol::methods::widgets::WidgetDescriptor;
use orkestr_storage::rusqlite::{params, OptionalExtension, Row};
use orkestr_storage::{Db, Migration, StorageError, StorageResult};

/// Dashboard migrations (owner `core`).
pub const DASHBOARD_MIGRATIONS: &[Migration] = &[Migration {
    id: "dashboards_0001_dashboards",
    owner: "core",
    up: "CREATE TABLE dashboards_dashboards (
            id         TEXT PRIMARY KEY NOT NULL,
            project_id TEXT,
            name       TEXT NOT NULL,
            widgets    TEXT NOT NULL,
            version    INTEGER NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
         );
         CREATE INDEX dashboards_dashboards_project ON dashboards_dashboards(project_id);",
    down: "DROP INDEX IF EXISTS dashboards_dashboards_project; DROP TABLE IF EXISTS dashboards_dashboards;",
}];

/// Max dashboard name length (chars).
pub const MAX_DASHBOARD_NAME: usize = 120;
/// Max rows a widget may span.
pub const MAX_WIDGET_ROWS: u32 = 24;
/// Max grid row a widget may start at.
pub const MAX_GRID_ROW: u32 = 512;
/// Max dashboards per scope (global or one project).
pub const MAX_DASHBOARDS_PER_SCOPE: usize = 100;

/// Normalized, validated dashboard name.
///
/// # Errors
/// Empty or too long.
pub fn validate_name(name: &str) -> StorageResult<String> {
    let n = name.trim();
    if n.is_empty() {
        return Err(StorageError::Invalid("dashboard name must not be empty".into()));
    }
    if n.chars().count() > MAX_DASHBOARD_NAME {
        return Err(StorageError::Invalid(format!("dashboard name longer than {MAX_DASHBOARD_NAME} characters")));
    }
    Ok(n.to_owned())
}

fn valid_instance_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Validates a layout against the widget catalogue.
///
/// * instance ids unique and `[A-Za-z0-9_-]{1,64}`;
/// * at most [`MAX_DASHBOARD_WIDGETS`] widgets; `x + w <= 12`, `1 <= h <= 24`;
/// * widget known in `catalog` and its config valid against its `configSchema`;
/// * config at most [`MAX_WIDGET_CONFIG_BYTES`];
/// * an unknown widget (plugin uninstalled/disabled) is accepted only when the
///   same instance is stored unchanged in `previous` (edits elsewhere must not
///   fail because a plugin went away).
///
/// # Errors
/// [`StorageError::Invalid`] naming the first offending instance.
pub fn validate_layout(
    widgets: &[WidgetInstance],
    catalog: &HashMap<String, WidgetDescriptor>,
    previous: Option<&[WidgetInstance]>,
) -> StorageResult<()> {
    if widgets.len() > MAX_DASHBOARD_WIDGETS {
        return Err(StorageError::Invalid(format!("at most {MAX_DASHBOARD_WIDGETS} widgets per dashboard")));
    }
    let mut seen = HashSet::new();
    for w in widgets {
        let at = |m: String| StorageError::Invalid(format!("widget {}: {m}", w.instance_id));
        if !valid_instance_id(&w.instance_id) {
            return Err(StorageError::Invalid(format!("invalid instance id {:?}", w.instance_id)));
        }
        if !seen.insert(w.instance_id.as_str()) {
            return Err(at("duplicate instance id".into()));
        }
        if w.w == 0 || w.w > DASHBOARD_COLUMNS || w.x >= DASHBOARD_COLUMNS || w.x + w.w > DASHBOARD_COLUMNS {
            return Err(at(format!("does not fit the {DASHBOARD_COLUMNS}-column grid")));
        }
        if w.h == 0 || w.h > MAX_WIDGET_ROWS || w.y > MAX_GRID_ROW {
            return Err(at(format!("height must be 1-{MAX_WIDGET_ROWS} rows")));
        }
        if let Some(t) = &w.title {
            if t.chars().count() > MAX_DASHBOARD_NAME {
                return Err(at("title too long".into()));
            }
        }
        let size = serde_json::to_vec(&w.config).map(|v| v.len()).unwrap_or(usize::MAX);
        if size > MAX_WIDGET_CONFIG_BYTES {
            return Err(at(format!("config larger than {MAX_WIDGET_CONFIG_BYTES} bytes")));
        }
        match catalog.get(&w.widget_id) {
            Some(desc) => {
                config_schema::validate(&desc.config_schema, &w.config).map_err(|e| at(format!("config {e}")))?;
            }
            None => {
                let unchanged = previous.is_some_and(|prev| {
                    prev.iter()
                        .any(|p| p.instance_id == w.instance_id && p.widget_id == w.widget_id && p.config == w.config)
                });
                if !unchanged {
                    return Err(at(format!("unknown widget {}", w.widget_id)));
                }
            }
        }
    }
    // Overlap check (grid cells are exclusive).
    for (i, a) in widgets.iter().enumerate() {
        for b in &widgets[i + 1..] {
            let overlap = a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
            if overlap {
                return Err(StorageError::Invalid(format!("widgets {} and {} overlap", a.instance_id, b.instance_id)));
            }
        }
    }
    Ok(())
}

/// A small, fail-closed JSON-schema subset for widget configs.
///
/// Supported: `type` (string or list), `properties`, `required`,
/// `additionalProperties` (bool or schema), `enum`, `const`, `minimum`,
/// `maximum`, `minLength`, `maxLength`, `pattern`, `items`, `minItems`,
/// `maxItems`. Annotations (`title`, `description`, `default`, `examples`,
/// `format`, `$schema`, `$id`, `readOnly`, `writeOnly`, `deprecated`, `x-*`)
/// are ignored. Any other keyword (e.g. `oneOf`, `$ref`) is rejected so a
/// schema is never silently under-enforced. Mirrored by the desktop's
/// `features/dashboards/configSchema.ts`.
pub mod config_schema {
    use serde_json::{Map, Value};

    const VALIDATION: &[&str] = &[
        "type",
        "properties",
        "required",
        "additionalProperties",
        "enum",
        "const",
        "minimum",
        "maximum",
        "minLength",
        "maxLength",
        "pattern",
        "items",
        "minItems",
        "maxItems",
    ];
    const ANNOTATIONS: &[&str] = &[
        "title",
        "description",
        "default",
        "examples",
        "format",
        "$schema",
        "$id",
        "readOnly",
        "writeOnly",
        "deprecated",
    ];
    const MAX_DEPTH: usize = 16;

    /// Validates `value` against `schema`. A `null`/absent schema accepts an
    /// empty object or null only.
    ///
    /// # Errors
    /// Human-readable message with a JSON pointer.
    pub fn validate(schema: &Value, value: &Value) -> Result<(), String> {
        if schema.is_null() {
            return match value {
                Value::Null => Ok(()),
                Value::Object(m) if m.is_empty() => Ok(()),
                _ => Err("not accepted: the widget has no config".into()),
            };
        }
        check(schema, value, "", 0)
    }

    fn type_matches(t: &str, v: &Value) -> Result<bool, String> {
        Ok(match t {
            "object" => v.is_object(),
            "array" => v.is_array(),
            "string" => v.is_string(),
            "boolean" => v.is_boolean(),
            "null" => v.is_null(),
            "number" => v.is_number(),
            "integer" => v.as_f64().is_some_and(|f| f.fract() == 0.0),
            other => return Err(format!("unsupported schema type {other:?}")),
        })
    }

    fn num(s: &Map<String, Value>, k: &str) -> Result<Option<f64>, String> {
        match s.get(k) {
            None => Ok(None),
            Some(v) => v.as_f64().map(Some).ok_or_else(|| format!("schema {k} must be a number")),
        }
    }

    fn count(s: &Map<String, Value>, k: &str) -> Result<Option<usize>, String> {
        match s.get(k) {
            None => Ok(None),
            Some(v) => v
                .as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .map(Some)
                .ok_or_else(|| format!("schema {k} must be a non-negative integer")),
        }
    }

    fn at(path: &str) -> &str {
        if path.is_empty() {
            "/"
        } else {
            path
        }
    }

    #[allow(clippy::too_many_lines)]
    fn check(schema: &Value, v: &Value, path: &str, depth: usize) -> Result<(), String> {
        if depth > MAX_DEPTH {
            return Err("schema nested too deeply".into());
        }
        let s = match schema {
            Value::Bool(true) => return Ok(()),
            Value::Bool(false) => return Err(format!("at {}: not allowed", at(path))),
            Value::Object(s) => s,
            _ => return Err("schema must be an object".into()),
        };
        for k in s.keys() {
            if !VALIDATION.contains(&k.as_str()) && !ANNOTATIONS.contains(&k.as_str()) && !k.starts_with("x-") {
                return Err(format!("uses unsupported schema keyword {k:?}"));
            }
        }
        if let Some(t) = s.get("type") {
            let ok = match t {
                Value::String(t) => type_matches(t, v)?,
                Value::Array(ts) => {
                    let mut any = false;
                    for t in ts {
                        let t = t.as_str().ok_or("schema type list must hold strings")?;
                        any |= type_matches(t, v)?;
                    }
                    any
                }
                _ => return Err("schema type must be a string or list".into()),
            };
            if !ok {
                return Err(format!("at {}: expected type {t}", at(path)));
            }
        }
        if let Some(e) = s.get("enum") {
            let list = e.as_array().ok_or("schema enum must be a list")?;
            if !list.contains(v) {
                return Err(format!("at {}: not one of the allowed values", at(path)));
            }
        }
        if let Some(c) = s.get("const") {
            if c != v {
                return Err(format!("at {}: must equal {c}", at(path)));
            }
        }
        if let Some(n) = v.as_f64() {
            if let Some(min) = num(s, "minimum")? {
                if n < min {
                    return Err(format!("at {}: must be >= {min}", at(path)));
                }
            }
            if let Some(max) = num(s, "maximum")? {
                if n > max {
                    return Err(format!("at {}: must be <= {max}", at(path)));
                }
            }
        }
        if let Some(text) = v.as_str() {
            let len = text.chars().count();
            if count(s, "minLength")?.is_some_and(|m| len < m) {
                return Err(format!("at {}: too short", at(path)));
            }
            if count(s, "maxLength")?.is_some_and(|m| len > m) {
                return Err(format!("at {}: too long", at(path)));
            }
            if let Some(p) = s.get("pattern") {
                let p = p.as_str().ok_or("schema pattern must be a string")?;
                let re = regex::RegexBuilder::new(p)
                    .size_limit(1 << 16)
                    .build()
                    .map_err(|_| "schema pattern is not a valid regex".to_owned())?;
                if !re.is_match(text) {
                    return Err(format!("at {}: does not match the pattern", at(path)));
                }
            }
        }
        if let Some(items) = v.as_array() {
            if count(s, "minItems")?.is_some_and(|m| items.len() < m) {
                return Err(format!("at {}: too few items", at(path)));
            }
            if count(s, "maxItems")?.is_some_and(|m| items.len() > m) {
                return Err(format!("at {}: too many items", at(path)));
            }
            if let Some(item_schema) = s.get("items") {
                for (i, item) in items.iter().enumerate() {
                    check(item_schema, item, &format!("{path}/{i}"), depth + 1)?;
                }
            }
        }
        if let Some(obj) = v.as_object() {
            let props = match s.get("properties") {
                None => None,
                Some(Value::Object(p)) => Some(p),
                Some(_) => return Err("schema properties must be an object".into()),
            };
            if let Some(req) = s.get("required") {
                for r in req.as_array().ok_or("schema required must be a list")? {
                    let r = r.as_str().ok_or("schema required must hold strings")?;
                    if !obj.contains_key(r) {
                        return Err(format!("at {}: missing required property {r:?}", at(path)));
                    }
                }
            }
            for (k, val) in obj {
                let child = format!("{path}/{k}");
                if let Some(ps) = props.and_then(|p| p.get(k)) {
                    check(ps, val, &child, depth + 1)?;
                } else {
                    match s.get("additionalProperties") {
                        None | Some(Value::Bool(true)) => {}
                        Some(Value::Bool(false)) => return Err(format!("at {}: unknown property {k:?}", at(path))),
                        Some(extra) => check(extra, val, &child, depth + 1)?,
                    }
                }
            }
        }
        Ok(())
    }
}

/// Dashboards store (stateless wrapper around the shared [`Db`]).
#[derive(Clone)]
pub struct DashboardsStore {
    db: Db,
}

impl std::fmt::Debug for DashboardsStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DashboardsStore").finish_non_exhaustive()
    }
}

/// What [`DashboardsStore::save`] writes (already validated by the caller).
#[derive(Debug, Clone)]
pub struct DashboardWrite {
    /// Existing dashboard (update) or `None` (create).
    pub id: Option<DashboardId>,
    /// Scope of a new dashboard (ignored on update).
    pub project_id: Option<ProjectId>,
    /// Name.
    pub name: String,
    /// Layout.
    pub widgets: Vec<WidgetInstance>,
    /// Optimistic concurrency.
    pub expected_version: Option<u32>,
}

fn parse_ts(raw: &str) -> Result<Timestamp, StorageError> {
    chrono::DateTime::parse_from_rfc3339(raw)
        .map(|t| t.with_timezone(&chrono::Utc))
        .map_err(|e| StorageError::Corrupt(format!("dashboard timestamp: {e}")))
}

struct RawRow {
    id: String,
    project_id: Option<String>,
    name: String,
    widgets: String,
    version: i64,
    created_at: String,
    updated_at: String,
}

const COLUMNS: &str = "id, project_id, name, widgets, version, created_at, updated_at";

fn raw(r: &Row<'_>) -> orkestr_storage::rusqlite::Result<RawRow> {
    Ok(RawRow {
        id: r.get(0)?,
        project_id: r.get(1)?,
        name: r.get(2)?,
        widgets: r.get(3)?,
        version: r.get(4)?,
        created_at: r.get(5)?,
        updated_at: r.get(6)?,
    })
}

impl RawRow {
    fn into_dashboard(self) -> StorageResult<Dashboard> {
        let corrupt = |e: String| StorageError::Corrupt(format!("dashboard {}: {e}", self.id));
        Ok(Dashboard {
            id: DashboardId::parse(&self.id).map_err(|e| corrupt(e.to_string()))?,
            project_id: self
                .project_id
                .as_deref()
                .map(ProjectId::parse)
                .transpose()
                .map_err(|e| corrupt(e.to_string()))?,
            name: self.name.clone(),
            widgets: serde_json::from_str(&self.widgets).map_err(|e| corrupt(e.to_string()))?,
            version: u32::try_from(self.version).map_err(|e| corrupt(e.to_string()))?,
            created_at: parse_ts(&self.created_at)?,
            updated_at: parse_ts(&self.updated_at)?,
        })
    }
}

impl DashboardsStore {
    /// Wraps a migrated database.
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Dashboards of `project` (plus global ones when `include_global`), or
    /// only global ones when `project` is `None`. Newest change first.
    pub async fn list(&self, project: Option<ProjectId>, include_global: bool) -> StorageResult<Vec<DashboardSummary>> {
        let pid = project.map(|p| p.to_string());
        let rows: Vec<RawRow> = self
            .db
            .call(move |c| {
                let sql = format!(
                    "SELECT {COLUMNS} FROM dashboards_dashboards
                     WHERE (?1 IS NOT NULL AND project_id = ?1) OR (project_id IS NULL AND (?1 IS NULL OR ?2))
                     ORDER BY updated_at DESC, id"
                );
                let mut stmt = c.prepare(&sql)?;
                let rows = stmt.query_map(params![pid, include_global], raw)?.collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            match r.into_dashboard() {
                Ok(d) => out.push(DashboardSummary {
                    id: d.id,
                    project_id: d.project_id,
                    name: d.name,
                    widget_count: u32::try_from(d.widgets.len()).unwrap_or(u32::MAX),
                    updated_at: d.updated_at,
                }),
                Err(e) => tracing::warn!(error = %e, "skipping unreadable dashboard"),
            }
        }
        Ok(out)
    }

    /// One dashboard.
    ///
    /// # Errors
    /// [`StorageError::NotFound`].
    pub async fn get(&self, id: DashboardId) -> StorageResult<Dashboard> {
        let key = id.to_string();
        let row = self
            .db
            .call(move |c| {
                Ok(c.query_row(&format!("SELECT {COLUMNS} FROM dashboards_dashboards WHERE id = ?1"), [&key], raw)
                    .optional()?)
            })
            .await?;
        row.ok_or_else(|| StorageError::NotFound(format!("dashboard {id}")))?.into_dashboard()
    }

    /// Creates or updates a dashboard (layout validated by the caller).
    ///
    /// # Errors
    /// Invalid name, not found, version conflict, scope full.
    pub async fn save(&self, w: DashboardWrite) -> StorageResult<Dashboard> {
        let name = validate_name(&w.name)?;
        let body = serde_json::to_string(&w.widgets)?;
        let now = chrono::Utc::now().to_rfc3339();
        let id = w.id.unwrap_or_default();
        let creating = w.id.is_none();
        let key = id.to_string();
        let pid = w.project_id.map(|p| p.to_string());
        let expected = w.expected_version;
        self.db
            .call(move |c| {
                let tx = c.transaction()?;
                if creating {
                    let n: i64 = tx.query_row(
                        "SELECT COUNT(*) FROM dashboards_dashboards WHERE project_id IS ?1",
                        [&pid],
                        |r| r.get(0),
                    )?;
                    if usize::try_from(n).unwrap_or(usize::MAX) >= MAX_DASHBOARDS_PER_SCOPE {
                        return Err(StorageError::Conflict(format!(
                            "at most {MAX_DASHBOARDS_PER_SCOPE} dashboards per scope"
                        )));
                    }
                    tx.execute(
                        "INSERT INTO dashboards_dashboards (id, project_id, name, widgets, version, created_at, updated_at)
                         VALUES (?1, ?2, ?3, ?4, 1, ?5, ?5)",
                        params![key, pid, name, body, now],
                    )?;
                } else {
                    let current: Option<i64> = tx
                        .query_row("SELECT version FROM dashboards_dashboards WHERE id = ?1", [&key], |r| r.get(0))
                        .optional()?;
                    let Some(current) = current else {
                        return Err(StorageError::NotFound(format!("dashboard {key}")));
                    };
                    if let Some(exp) = expected {
                        if i64::from(exp) != current {
                            return Err(StorageError::Conflict(format!(
                                "dashboard changed elsewhere (version {current}, expected {exp}); reload and retry"
                            )));
                        }
                    }
                    tx.execute(
                        "UPDATE dashboards_dashboards SET name = ?2, widgets = ?3, version = version + 1, updated_at = ?4
                         WHERE id = ?1",
                        params![key, name, body, now],
                    )?;
                }
                tx.commit()?;
                Ok(())
            })
            .await?;
        self.get(id).await
    }

    /// Deletes a dashboard, returning what was removed.
    ///
    /// # Errors
    /// [`StorageError::NotFound`].
    pub async fn delete(&self, id: DashboardId) -> StorageResult<Dashboard> {
        let existing = self.get(id).await?;
        let key = id.to_string();
        self.db.call(move |c| Ok(c.execute("DELETE FROM dashboards_dashboards WHERE id = ?1", [&key])?)).await?;
        Ok(existing)
    }
}

#[cfg(test)]
mod tests {
    use orkestr_domain::{DataBinding, WidgetSize};
    use orkestr_protocol::methods::widgets::WidgetSource;
    use serde_json::json;

    use super::*;

    async fn store() -> (tempfile::TempDir, Db, DashboardsStore) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path().join("d.db")).unwrap();
        db.migrate(crate::migrations::all()).await.unwrap();
        (dir, db.clone(), DashboardsStore::new(db))
    }

    fn inst(id: &str, widget: &str, x: u32, y: u32, w: u32, h: u32, config: serde_json::Value) -> WidgetInstance {
        WidgetInstance { instance_id: id.into(), widget_id: widget.into(), x, y, w, h, config, title: None }
    }

    fn write(
        id: Option<DashboardId>,
        project: Option<ProjectId>,
        name: &str,
        widgets: Vec<WidgetInstance>,
    ) -> DashboardWrite {
        DashboardWrite { id, project_id: project, name: name.into(), widgets, expected_version: None }
    }

    #[tokio::test]
    async fn crud_per_scope() {
        let (_d, _db, s) = store().await;
        let p = ProjectId::new();
        let g = s
            .save(write(None, None, " Global ", vec![inst("a", "builtin:note", 0, 0, 4, 3, json!({}))]))
            .await
            .unwrap();
        assert_eq!(g.name, "Global");
        assert_eq!(g.version, 1);
        let pd = s.save(write(None, Some(p), "Proj", vec![])).await.unwrap();
        assert_eq!(pd.project_id, Some(p));

        assert_eq!(s.list(None, true).await.unwrap().len(), 1, "no project → global only");
        assert_eq!(s.list(Some(p), true).await.unwrap().len(), 2);
        let only = s.list(Some(p), false).await.unwrap();
        assert_eq!(only.len(), 1);
        assert_eq!(only[0].id, pd.id);
        assert_eq!(s.list(Some(ProjectId::new()), false).await.unwrap().len(), 0);

        let mut upd =
            write(Some(g.id), None, "Renamed", vec![inst("a", "builtin:note", 2, 1, 6, 4, json!({"text": "hi"}))]);
        upd.expected_version = Some(1);
        let g2 = s.save(upd.clone()).await.unwrap();
        assert_eq!((g2.version, g2.name.as_str(), g2.widgets[0].x), (2, "Renamed", 2));
        assert_eq!(g2.created_at, g.created_at);
        // stale version → conflict
        assert!(matches!(s.save(upd).await, Err(StorageError::Conflict(_))));
        assert!(matches!(
            s.save(write(Some(DashboardId::new()), None, "x", vec![])).await,
            Err(StorageError::NotFound(_))
        ));
        assert!(matches!(s.save(write(None, None, "  ", vec![])).await, Err(StorageError::Invalid(_))));

        let del = s.delete(g.id).await.unwrap();
        assert_eq!(del.id, g.id);
        assert!(matches!(s.get(g.id).await, Err(StorageError::NotFound(_))));
        assert!(matches!(s.delete(g.id).await, Err(StorageError::NotFound(_))));
    }

    #[tokio::test]
    async fn migration_reverts_cleanly() {
        let (_d, db, s) = store().await;
        s.save(write(None, None, "x", vec![])).await.unwrap();
        // Revert everything back to (and excluding) the dashboards migration.
        let all = crate::migrations::all();
        let pos = all.iter().position(|m| m.id == DASHBOARD_MIGRATIONS[0].id).unwrap();
        let n = all.len() - pos;
        let reverted = db.call(move |c| orkestr_storage::Migrator::rollback_last(c, n)).await.unwrap();
        assert!(reverted.iter().any(|r| r == "dashboards_0001_dashboards"));
        let left: i64 = db
            .call(|c| {
                Ok(c.query_row("SELECT COUNT(*) FROM sqlite_master WHERE name LIKE 'dashboards_%'", [], |r| r.get(0))?)
            })
            .await
            .unwrap();
        assert_eq!(left, 0);
        // and re-applies
        db.migrate(crate::migrations::all()).await.unwrap();
        assert!(s.list(None, true).await.unwrap().is_empty());
    }

    fn catalog() -> HashMap<String, WidgetDescriptor> {
        let d = WidgetDescriptor {
            id: "builtin:note".into(),
            title: "Note".into(),
            source: WidgetSource::BuiltIn,
            sizes: vec![WidgetSize { w: 4, h: 3 }],
            data_binding: DataBinding::default(),
            config_schema: json!({
                "type": "object",
                "properties": {
                    "text": {"type": "string", "maxLength": 5, "title": "Text"},
                    "tone": {"type": "string", "enum": ["a", "b"]},
                    "n": {"type": "integer", "minimum": 1, "maximum": 3}
                },
                "required": ["text"],
                "additionalProperties": false
            }),
        };
        HashMap::from([(d.id.clone(), d)])
    }

    #[test]
    fn layout_validation() {
        let c = catalog();
        let ok = inst("a", "builtin:note", 0, 0, 4, 3, json!({"text": "hi", "tone": "a", "n": 2}));
        assert!(validate_layout(std::slice::from_ref(&ok), &c, None).is_ok());
        let bad = |w: WidgetInstance| validate_layout(&[w], &c, None).unwrap_err().to_string();
        assert!(bad(inst("a", "builtin:note", 0, 0, 4, 3, json!({}))).contains("missing required"));
        assert!(bad(inst("a", "builtin:note", 0, 0, 4, 3, json!({"text": "toolong"}))).contains("too long"));
        assert!(bad(inst("a", "builtin:note", 0, 0, 4, 3, json!({"text": "a", "tone": "z"}))).contains("allowed"));
        assert!(bad(inst("a", "builtin:note", 0, 0, 4, 3, json!({"text": "a", "n": 1.5}))).contains("integer"));
        assert!(bad(inst("a", "builtin:note", 0, 0, 4, 3, json!({"text": "a", "x": 1}))).contains("unknown property"));
        assert!(bad(inst("a", "builtin:note", 10, 0, 4, 3, json!({"text": "a"}))).contains("grid"));
        assert!(bad(inst("a", "builtin:note", 0, 0, 4, 0, json!({"text": "a"}))).contains("height"));
        assert!(bad(inst("a b", "builtin:note", 0, 0, 4, 3, json!({"text": "a"}))).contains("instance id"));
        assert!(bad(inst("a", "pkg/gone", 0, 0, 4, 3, json!({}))).contains("unknown widget"));
        // unknown widget kept unchanged from the previous version is fine
        let gone = inst("g", "pkg/gone", 4, 0, 4, 3, json!({"k": 1}));
        assert!(validate_layout(&[ok.clone(), gone.clone()], &c, Some(std::slice::from_ref(&gone))).is_ok());
        let mut changed = gone.clone();
        changed.config = json!({"k": 2});
        assert!(validate_layout(&[changed], &c, Some(&[gone])).is_err());
        // overlap + duplicates
        let over = inst("b", "builtin:note", 2, 1, 4, 3, json!({"text": "a"}));
        assert!(validate_layout(&[ok.clone(), over], &c, None).unwrap_err().to_string().contains("overlap"));
        assert!(validate_layout(&[ok.clone(), ok], &c, None).unwrap_err().to_string().contains("duplicate"));
    }

    #[test]
    fn schema_subset_fails_closed() {
        use config_schema::validate;
        assert!(validate(&json!({"oneOf": []}), &json!({})).unwrap_err().contains("unsupported"));
        assert!(validate(&json!({"$ref": "#/x"}), &json!({})).is_err());
        assert!(validate(&serde_json::Value::Null, &json!({})).is_ok());
        assert!(validate(&serde_json::Value::Null, &json!({"a": 1})).is_err());
        let s =
            json!({"type": "array", "items": {"type": "string", "pattern": "^[a-z]+$"}, "maxItems": 2, "x-ui": "tags"});
        assert!(validate(&s, &json!(["ab", "cd"])).is_ok());
        assert!(validate(&s, &json!(["ab", "C"])).unwrap_err().contains("/1"));
        assert!(validate(&s, &json!(["a", "b", "c"])).is_err());
        assert!(validate(&json!({"type": ["string", "null"]}), &json!(null)).is_ok());
    }
}
