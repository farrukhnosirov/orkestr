//! Data backups taken before an update is installed and before a new version
//! migrates the data on its first start.
//!
//! A backup is one zip in `<data_dir>/backups/` holding:
//! * every SQLite database under the data dir, copied with the **SQLite
//!   online-backup API** (consistent even while the core is writing; WAL
//!   content included), and
//! * small settings files (`*.json`, ≤ 1 MiB) of the data and config dirs.
//!
//! Not included: content-addressed objects (`objects/`, append-only, never
//! rewritten by migrations), downloaded runtimes (`vscode-server/`,
//! `extensions/`), logs, caches, and secrets (`*.key`, `*.token`): secrets
//! stay in the OS keyring / their single file and are never duplicated.
//!
//! Backups are never restored automatically. Retention keeps the newest N.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{UpdateError, UpdateResult};

/// Folder (inside the data dir) holding the backups.
pub const BACKUPS_DIR: &str = "backups";
/// Name of the metadata entry inside each zip.
pub const META_ENTRY: &str = "backup.json";
const PREFIX: &str = "orkestr-backup-";
/// Largest non-database file copied into a backup.
const MAX_SMALL_FILE: u64 = 1024 * 1024;
/// Directories under the data dir that are never walked.
const SKIP_DIRS: &[&str] = &[
    BACKUPS_DIR,
    "objects",
    "blobs",
    "vscode-server",
    "extensions",
    "logs",
    "cache",
    "node_modules",
    "staging",
    "downloads",
    // Browser profiles (WebView2 keeps its own SQLite files locked while running).
    "wv2",
    "EBWebView",
    "webview",
];
/// Databases are looked for in the data dir and its direct subfolders only
/// (`orkestr.db`, `sync/sync.db`, …); deeper `.db` files belong to embedded tools.
const MAX_DB_DEPTH: usize = 1;
/// Files never copied (runtime state and secrets).
const SKIP_FILES: &[&str] = &["core.json", "core.token", "session.lock"];

/// Why a backup was taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BackupReason {
    /// Right before an update installer is launched.
    PreUpdate,
    /// On the first start of a new version, before its migrations run.
    PreMigration,
    /// Requested by the user.
    Manual,
}

impl BackupReason {
    fn slug(self) -> &'static str {
        match self {
            Self::PreUpdate => "pre-update",
            Self::PreMigration => "pre-migration",
            Self::Manual => "manual",
        }
    }
}

/// Metadata of one backup (also stored as `backup.json` inside the zip).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupInfo {
    /// Absolute path of the zip.
    pub path: PathBuf,
    /// Why it was taken.
    pub reason: BackupReason,
    /// When.
    pub created_at: DateTime<Utc>,
    /// Version whose data this is.
    pub from_version: String,
    /// Version about to be installed / started, if any.
    pub to_version: Option<String>,
    /// Database files included (paths relative to the data dir).
    pub databases: Vec<String>,
    /// Databases that could not be copied (busy/locked by another program).
    #[serde(default)]
    pub skipped: Vec<String>,
    /// Other files included.
    pub files: usize,
    /// Zip size in bytes.
    pub size_bytes: u64,
}

/// What to back up.
#[derive(Debug, Clone)]
pub struct BackupRequest<'a> {
    /// Orkestr data dir (`Paths::data_dir`).
    pub data_dir: &'a Path,
    /// Orkestr config dir (`Paths::config_dir`), copied under `config/`.
    pub config_dir: Option<&'a Path>,
    /// Why.
    pub reason: BackupReason,
    /// Version whose data this is.
    pub from_version: &'a str,
    /// Target version.
    pub to_version: Option<&'a str>,
    /// Keep this many newest backups (≥ 1).
    pub retention: usize,
}

/// `<data_dir>/backups`.
#[must_use]
pub fn backups_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(BACKUPS_DIR)
}

fn is_db(path: &Path) -> bool {
    matches!(path.extension().and_then(|e| e.to_str()), Some("db" | "sqlite" | "sqlite3"))
}

fn is_secret(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".key") || lower.ends_with(".token") || lower.ends_with(".pem")
}

/// Walks `root` (depth ≤ 4) collecting `(absolute, relative)` files to back up.
fn collect(
    root: &Path,
    rel: &Path,
    depth: usize,
    dbs: &mut Vec<(PathBuf, String)>,
    small: &mut Vec<(PathBuf, String)>,
) {
    let Ok(entries) = std::fs::read_dir(root.join(rel)) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(ft) = entry.file_type() else { continue };
        let rel_path = rel.join(&name);
        if ft.is_dir() {
            if depth < 4 && !SKIP_DIRS.contains(&name.as_str()) {
                collect(root, &rel_path, depth + 1, dbs, small);
            }
            continue;
        }
        if !ft.is_file() || SKIP_FILES.contains(&name.as_str()) || is_secret(&name) {
            continue;
        }
        let rel_str = rel_path.to_string_lossy().replace('\\', "/");
        let abs = entry.path();
        if is_db(&abs) {
            if depth <= MAX_DB_DEPTH {
                dbs.push((abs, rel_str));
            }
        } else if name.ends_with(".json") && entry.metadata().is_ok_and(|m| m.len() <= MAX_SMALL_FILE) {
            small.push((abs, rel_str));
        }
    }
}

/// Copies a live SQLite database with the online-backup API.
pub fn backup_sqlite(src: &Path, dest: &Path) -> UpdateResult<()> {
    // Read-only: a backup never changes the source. Busy/locked sources are
    // retried until BACKUP_DEADLINE, then reported (never an endless wait).
    let from = rusqlite::Connection::open_with_flags(
        src,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    from.busy_timeout(BUSY_TIMEOUT)?;
    let mut to = rusqlite::Connection::open(dest)?;
    {
        let backup = rusqlite::backup::Backup::new(&from, &mut to)?;
        let deadline = std::time::Instant::now() + BACKUP_DEADLINE;
        loop {
            match backup.step(256)? {
                rusqlite::backup::StepResult::Done => break,
                rusqlite::backup::StepResult::More => {}
                // Busy / Locked (and future variants): retry until the deadline.
                _ => {
                    if std::time::Instant::now() > deadline {
                        return Err(UpdateError::Io(format!("{} stayed busy; backup aborted", src.display())));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            }
        }
    }
    // A self-contained file (no -wal next to it inside the zip).
    to.pragma_update(None, "journal_mode", "DELETE")?;
    Ok(())
}

/// Busy timeout of the backup connections.
const BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// A backup never blocks startup or an install for longer than this.
const BACKUP_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

/// Takes a backup and prunes old ones. Fails without leaving partial files.
pub fn create_backup(req: &BackupRequest<'_>) -> UpdateResult<BackupInfo> {
    let dir = backups_dir(req.data_dir);
    std::fs::create_dir_all(&dir)?;
    let created_at = Utc::now();
    let stamp = created_at.format("%Y%m%dT%H%M%S%3fZ");
    let to = req.to_version.map(|v| format!("-to-v{v}")).unwrap_or_default();
    let base = format!("{PREFIX}{stamp}-{}-v{}{to}", req.reason.slug(), req.from_version);
    let staging = dir.join(format!(".{base}.tmp"));
    let zip_path = dir.join(format!("{base}.zip"));
    let result = (|| -> UpdateResult<BackupInfo> {
        std::fs::create_dir_all(&staging)?;
        let mut dbs = Vec::new();
        let mut small = Vec::new();
        collect(req.data_dir, Path::new(""), 0, &mut dbs, &mut small);
        let mut config_files = Vec::new();
        if let Some(cfg) = req.config_dir.filter(|c| c.is_dir() && *c != req.data_dir) {
            let mut cfg_dbs = Vec::new();
            collect(cfg, Path::new(""), 1, &mut cfg_dbs, &mut config_files);
        }
        let mut databases = Vec::new();
        let mut skipped = Vec::new();
        let mut copies: Vec<(PathBuf, String)> = Vec::new();
        for (i, (abs, rel)) in dbs.iter().enumerate() {
            let tmp = staging.join(format!("db{i}.sqlite"));
            match backup_sqlite(abs, &tmp) {
                Ok(()) => {
                    copies.push((tmp, format!("data/{rel}")));
                    databases.push(rel.clone());
                }
                // The core database is the point of the backup: never skip it.
                Err(e) if rel == crate::startup::DATABASE_FILE => {
                    return Err(UpdateError::Io(format!("backing up {rel}: {e}")));
                }
                Err(e) => {
                    tracing::warn!(db = %rel, error = %e, "database skipped in backup");
                    let _ = std::fs::remove_file(&tmp);
                    skipped.push(rel.clone());
                }
            }
        }
        let files = small.len() + config_files.len();
        copies.extend(small.into_iter().map(|(a, r)| (a, format!("data/{r}"))));
        copies.extend(config_files.into_iter().map(|(a, r)| (a, format!("config/{r}"))));

        let mut info = BackupInfo {
            path: zip_path.clone(),
            reason: req.reason,
            created_at,
            from_version: req.from_version.to_owned(),
            to_version: req.to_version.map(str::to_owned),
            databases,
            skipped,
            files,
            size_bytes: 0,
        };
        let part = dir.join(format!(".{base}.zip.part"));
        {
            let file = std::fs::File::create(&part)?;
            let mut zip = zip::ZipWriter::new(std::io::BufWriter::new(file));
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .large_file(true);
            zip.start_file(META_ENTRY, opts)?;
            zip.write_all(&serde_json::to_vec_pretty(&info).map_err(|e| UpdateError::Io(e.to_string()))?)?;
            for (abs, name) in &copies {
                zip.start_file(name.as_str(), opts)?;
                let mut f = std::fs::File::open(abs)?;
                std::io::copy(&mut f, &mut zip)?;
            }
            let mut inner = zip.finish()?;
            inner.flush()?;
        }
        std::fs::rename(&part, &zip_path)?;
        info.size_bytes = std::fs::metadata(&zip_path)?.len();
        Ok(info)
    })();
    let _ = std::fs::remove_dir_all(&staging);
    let _ = std::fs::remove_file(dir.join(format!(".{base}.zip.part")));
    let info = result?;
    if let Err(e) = prune(req.data_dir, req.retention.max(1)) {
        tracing::warn!(error = %e, "could not prune old backups");
    }
    tracing::info!(path = %info.path.display(), dbs = info.databases.len(), bytes = info.size_bytes, "data backup created");
    Ok(info)
}

/// Lists backups, newest first (reads each zip's `backup.json`).
pub fn list_backups(data_dir: &Path) -> UpdateResult<Vec<BackupInfo>> {
    let dir = backups_dir(data_dir);
    let Ok(entries) = std::fs::read_dir(&dir) else { return Ok(Vec::new()) };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if !(name.starts_with(PREFIX) && name.ends_with(".zip")) {
            continue;
        }
        match read_meta(&path) {
            Ok(mut info) => {
                info.path = path.clone();
                info.size_bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
                out.push(info);
            }
            Err(e) => tracing::warn!(path = %path.display(), error = %e, "unreadable backup"),
        }
    }
    out.sort_by_key(|b| std::cmp::Reverse(b.created_at));
    Ok(out)
}

fn read_meta(path: &Path) -> UpdateResult<BackupInfo> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path)?)?;
    let entry = zip.by_name(META_ENTRY)?;
    serde_json::from_reader(entry).map_err(|e| UpdateError::Io(e.to_string()))
}

/// Deletes all but the `keep` newest backups. Returns how many were removed.
pub fn prune(data_dir: &Path, keep: usize) -> UpdateResult<usize> {
    // Leftovers of an interrupted backup (`.<name>.tmp` dirs, `.<name>.zip.part`).
    if let Ok(entries) = std::fs::read_dir(backups_dir(data_dir)) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with(&format!(".{PREFIX}")) {
                let _ =
                    if e.path().is_dir() { std::fs::remove_dir_all(e.path()) } else { std::fs::remove_file(e.path()) };
            }
        }
    }
    let all = list_backups(data_dir)?;
    let mut removed = 0;
    for old in all.iter().skip(keep.max(1)) {
        std::fs::remove_file(&old.path)?;
        removed += 1;
    }
    Ok(removed)
}
