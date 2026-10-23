//! First start of a (new) version: detect upgrades/downgrades, protect data
//! written by a newer Orkestr, and back the data up before migrations run.
//!
//! Runs in the desktop shell **before** the embedded core opens the database.
//! The storage migrator also refuses unknown migrations (orkestr-storage);
//! this check turns that into a clear, localized message with the backups
//! location instead of a generic startup failure.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::backup::{create_backup, BackupInfo, BackupReason, BackupRequest};
use crate::error::{UpdateError, UpdateResult};
use crate::manifest::parse_version;

/// Marker file (data dir) with the version that last opened the data.
pub const VERSION_MARKER: &str = "app-version.json";
/// SQLite database of the core (`Paths::database_file`).
pub const DATABASE_FILE: &str = "orkestr.db";

/// Contents of [`VERSION_MARKER`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionMarker {
    /// Version that last started on this data.
    pub version: String,
    /// The version before it (rollback information).
    pub previous_version: Option<String>,
    /// When `version` first started.
    pub since: DateTime<Utc>,
}

/// How this start relates to the previous one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum StartKind {
    /// No marker yet (fresh install, or data from before markers existed).
    FirstRun,
    /// Same version as last time.
    Same,
    /// A newer version starts on older data.
    Upgraded {
        /// Previous version.
        from: String,
    },
    /// An older version starts (manual rollback) and the schema is compatible.
    Downgraded {
        /// Previous version.
        from: String,
    },
}

/// Schema compatibility of the data with this build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "state")]
pub enum SchemaState {
    /// No database yet.
    NoDatabase,
    /// Every applied migration is known; `pending` will run at startup.
    Compatible {
        /// Migrations this build will apply.
        pending: usize,
    },
    /// The database has migrations this build does not know (newer Orkestr).
    Newer {
        /// Unknown migration ids.
        unknown: Vec<String>,
    },
}

/// Result of [`prepare_start`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupReport {
    /// This build's version.
    pub version: String,
    /// Upgrade / downgrade / same.
    pub start: StartKind,
    /// The marker as found (rollback info: previous version and since when).
    pub previous: Option<VersionMarker>,
    /// Schema compatibility.
    pub schema: SchemaState,
    /// Backup taken before migrations, if any.
    pub backup: Option<BackupInfo>,
    /// Why a needed backup could not be taken (startup continues).
    pub backup_error: Option<String>,
}

/// Reads the version marker.
#[must_use]
pub fn read_marker(data_dir: &Path) -> Option<VersionMarker> {
    let bytes = std::fs::read(data_dir.join(VERSION_MARKER)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write_marker(data_dir: &Path, marker: &VersionMarker) -> UpdateResult<()> {
    std::fs::create_dir_all(data_dir)?;
    let tmp = data_dir.join(format!("{VERSION_MARKER}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec_pretty(marker).map_err(|e| UpdateError::Io(e.to_string()))?)?;
    std::fs::rename(&tmp, data_dir.join(VERSION_MARKER))?;
    Ok(())
}

/// Compares the applied migrations of `db` with the ids this build knows.
pub fn inspect_schema(db: &Path, known: &[&str]) -> UpdateResult<SchemaState> {
    if !db.is_file() {
        return Ok(SchemaState::NoDatabase);
    }
    let conn = rusqlite::Connection::open_with_flags(
        db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    let has_table: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='_orkestr_migrations')",
        [],
        |r| r.get(0),
    )?;
    if !has_table {
        return Ok(SchemaState::Compatible { pending: known.len() });
    }
    let mut stmt = conn.prepare("SELECT id FROM _orkestr_migrations ORDER BY seq")?;
    let applied = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
    let unknown: Vec<String> = applied.iter().filter(|id| !known.contains(&id.as_str())).cloned().collect();
    if !unknown.is_empty() {
        return Ok(SchemaState::Newer { unknown });
    }
    let pending = known.iter().filter(|k| !applied.iter().any(|a| a == *k)).count();
    Ok(SchemaState::Compatible { pending })
}

/// Inputs of [`prepare_start`].
#[derive(Debug, Clone)]
pub struct StartRequest<'a> {
    /// Data dir.
    pub data_dir: &'a Path,
    /// Config dir (included in backups).
    pub config_dir: Option<&'a Path>,
    /// This build's version.
    pub version: &'a Version,
    /// Every migration id this build knows (`orkestr_core::migrations::all()`).
    pub known_migrations: &'a [&'a str],
    /// Backups to keep.
    pub retention: usize,
}

/// Runs before the core starts. Errors with [`UpdateError::DatabaseNewer`]
/// when the data was written by a newer Orkestr (downgrade protection) —
/// the data is then left untouched.
pub fn prepare_start(req: &StartRequest<'_>) -> UpdateResult<StartupReport> {
    let previous = read_marker(req.data_dir);
    let db = database_path(req.data_dir);
    let schema = inspect_schema(&db, req.known_migrations)?;
    if let SchemaState::Newer { unknown } = &schema {
        let by = previous.as_ref().map_or_else(|| "a newer Orkestr".to_owned(), |m| format!("Orkestr {}", m.version));
        tracing::error!(?unknown, %by, "data schema is newer than this build; refusing to start");
        return Err(UpdateError::DatabaseNewer(by));
    }
    let current = req.version.to_string();
    let start = match previous.as_ref().map(|m| (m, parse_version(&m.version))) {
        None => StartKind::FirstRun,
        Some((m, Ok(prev))) if prev < *req.version => StartKind::Upgraded { from: m.version.clone() },
        Some((m, Ok(prev))) if prev > *req.version => StartKind::Downgraded { from: m.version.clone() },
        Some((m, Err(_))) if m.version != current => StartKind::Upgraded { from: m.version.clone() },
        Some(_) => StartKind::Same,
    };
    let pending = matches!(schema, SchemaState::Compatible { pending } if pending > 0);
    let needs_backup =
        db.is_file() && (pending || matches!(start, StartKind::Upgraded { .. } | StartKind::Downgraded { .. }));
    let (backup, backup_error) = if needs_backup {
        let from = previous.as_ref().map_or("unknown", |m| m.version.as_str());
        match create_backup(&BackupRequest {
            data_dir: req.data_dir,
            config_dir: req.config_dir,
            reason: BackupReason::PreMigration,
            from_version: from,
            to_version: Some(&current),
            retention: req.retention,
        }) {
            Ok(b) => (Some(b), None),
            Err(e) => {
                tracing::error!(error = %e, "pre-migration backup failed; continuing");
                (None, Some(e.to_string()))
            }
        }
    } else {
        (None, None)
    };
    let marker = match &previous {
        Some(m) if m.version == current => m.clone(),
        Some(m) => {
            VersionMarker { version: current.clone(), previous_version: Some(m.version.clone()), since: Utc::now() }
        }
        None => VersionMarker { version: current.clone(), previous_version: None, since: Utc::now() },
    };
    if previous.as_ref() != Some(&marker) {
        write_marker(req.data_dir, &marker)?;
    }
    Ok(StartupReport { version: current, start, previous, schema, backup, backup_error })
}

/// `<data_dir>/orkestr.db`.
#[must_use]
pub fn database_path(data_dir: &Path) -> PathBuf {
    data_dir.join(DATABASE_FILE)
}
