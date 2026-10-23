//! Data backups (SQLite online-backup API, retention) and the first-start
//! checks of a new version (upgrade detection, pre-migration backup,
//! downgrade protection).

use std::io::Read as _;
use std::path::Path;

use orkestr_updater::backup::{list_backups, prune};
use orkestr_updater::startup::{read_marker, SchemaState};
use orkestr_updater::{
    create_backup, prepare_start, BackupReason, BackupRequest, StartKind, StartRequest, UpdateError,
};
use semver::Version;

const KNOWN: &[&str] = &["core_0001_meta", "storage_0001_projects"];

/// A live WAL database the way the core leaves it (connection kept open).
fn make_db(path: &Path, migrations: &[&str]) -> rusqlite::Connection {
    let c = rusqlite::Connection::open(path).unwrap();
    c.pragma_update(None, "journal_mode", "WAL").unwrap();
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS _orkestr_migrations (seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE,
           owner TEXT NOT NULL, checksum TEXT NOT NULL, down_sql TEXT NOT NULL, applied_at TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS marker (v TEXT);",
    )
    .unwrap();
    for m in migrations {
        c.execute(
            "INSERT INTO _orkestr_migrations (id, owner, checksum, down_sql, applied_at) VALUES (?1,'t','x','x','now')",
            [m],
        )
        .unwrap();
    }
    c.execute("INSERT INTO marker VALUES ('keep me')", []).unwrap();
    c
}

fn zip_entries(path: &Path) -> Vec<String> {
    let z = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    z.file_names().map(str::to_owned).collect()
}

#[test]
fn backup_copies_live_databases_and_settings_but_not_secrets() {
    let home = tempfile::tempdir().unwrap();
    let data = home.path().join("data");
    let config = home.path().join("config");
    std::fs::create_dir_all(data.join("sync")).unwrap();
    std::fs::create_dir_all(data.join("remote")).unwrap();
    std::fs::create_dir_all(data.join("vscode-server")).unwrap();
    std::fs::create_dir_all(&config).unwrap();
    let _live = make_db(&data.join("orkestr.db"), KNOWN); // stays open (WAL not checkpointed)
    let _sync = make_db(&data.join("sync").join("sync.db"), &[]);
    std::fs::write(data.join("claude-code.json"), "{}").unwrap();
    std::fs::write(data.join("core.token"), "secret").unwrap();
    std::fs::write(data.join("remote").join("cik.key"), "secret").unwrap();
    std::fs::write(data.join("vscode-server").join("big.json"), "{}").unwrap();
    // A WebView2 profile inside the data dir (ORKESTR_HOME setups): its DBs are locked
    // by the browser process and must not be touched (found by the installer e2e run).
    std::fs::create_dir_all(data.join("wv2").join("EBWebView").join("Default")).unwrap();
    let browser = rusqlite::Connection::open(data.join("wv2").join("EBWebView").join("Default").join("x.db")).unwrap();
    browser.execute_batch("PRAGMA locking_mode=EXCLUSIVE; CREATE TABLE t(v); INSERT INTO t VALUES (1);").unwrap();
    // A deep tool database is not ours either.
    std::fs::create_dir_all(data.join("sync").join("deep")).unwrap();
    drop(rusqlite::Connection::open(data.join("sync").join("deep").join("tool.db")).unwrap());
    std::fs::write(config.join("updates.json"), "{}").unwrap();

    let info = create_backup(&BackupRequest {
        data_dir: &data,
        config_dir: Some(&config),
        reason: BackupReason::PreUpdate,
        from_version: "0.1.0",
        to_version: Some("0.1.1"),
        retention: 5,
    })
    .unwrap();
    assert!(info.path.starts_with(data.join("backups")));
    assert!(info.size_bytes > 0);
    let mut dbs = info.databases.clone();
    dbs.sort();
    assert_eq!(dbs, vec!["orkestr.db", "sync/sync.db"]);
    assert!(info.skipped.is_empty());
    drop(browser);
    let names = zip_entries(&info.path);
    for want in ["backup.json", "data/orkestr.db", "data/sync/sync.db", "data/claude-code.json", "config/updates.json"]
    {
        assert!(names.contains(&want.to_owned()), "{want} missing from {names:?}");
    }
    for never in ["core.token", "cik.key", "big.json"] {
        assert!(!names.iter().any(|n| n.ends_with(never)), "{never} must not be backed up");
    }

    // The copied DB is consistent and contains the row that is still in the WAL.
    let mut z = zip::ZipArchive::new(std::fs::File::open(&info.path).unwrap()).unwrap();
    let mut bytes = Vec::new();
    z.by_name("data/orkestr.db").unwrap().read_to_end(&mut bytes).unwrap();
    let restored = home.path().join("restored.db");
    std::fs::write(&restored, bytes).unwrap();
    let c = rusqlite::Connection::open(&restored).unwrap();
    let v: String = c.query_row("SELECT v FROM marker", [], |r| r.get(0)).unwrap();
    assert_eq!(v, "keep me");

    let listed = list_backups(&data).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].reason, BackupReason::PreUpdate);
    assert_eq!(listed[0].to_version.as_deref(), Some("0.1.1"));
    // No staging leftovers.
    let leftovers: Vec<_> = std::fs::read_dir(data.join("backups"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with('.'))
        .collect();
    assert!(leftovers.is_empty());
}

#[test]
fn retention_keeps_the_newest_backups() {
    let home = tempfile::tempdir().unwrap();
    let _db = make_db(&home.path().join("orkestr.db"), KNOWN);
    for _ in 0..4 {
        create_backup(&BackupRequest {
            data_dir: home.path(),
            config_dir: None,
            reason: BackupReason::Manual,
            from_version: "0.1.0",
            to_version: None,
            retention: 3,
        })
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let list = list_backups(home.path()).unwrap();
    assert_eq!(list.len(), 3, "retention 3");
    assert!(list.windows(2).all(|w| w[0].created_at >= w[1].created_at), "newest first");
    assert_eq!(prune(home.path(), 1).unwrap(), 2);
    assert_eq!(list_backups(home.path()).unwrap().len(), 1);
}

#[test]
fn first_start_of_a_new_version_backs_up_before_migrations() {
    let home = tempfile::tempdir().unwrap();
    let data = home.path();
    let v010s = &Version::new(0, 1, 0);
    let v011s = &Version::new(0, 1, 1);
    fn req<'a>(data: &'a Path, v: &'a Version, known: &'a [&'a str]) -> StartRequest<'a> {
        StartRequest { data_dir: data, config_dir: None, version: v, known_migrations: known, retention: 5 }
    }

    // Fresh install: nothing to back up.
    let r = prepare_start(&req(data, v010s, KNOWN)).unwrap();
    assert_eq!(r.start, StartKind::FirstRun);
    assert_eq!(r.schema, SchemaState::NoDatabase);
    assert!(r.backup.is_none());
    assert_eq!(read_marker(data).unwrap().version, "0.1.0");

    // The core created the DB with all known migrations.
    drop(make_db(&data.join("orkestr.db"), KNOWN));
    let r = prepare_start(&req(data, v010s, KNOWN)).unwrap();
    assert_eq!(r.start, StartKind::Same);
    assert_eq!(r.schema, SchemaState::Compatible { pending: 0 });
    assert!(r.backup.is_none(), "same version, nothing pending: no backup");

    // 0.1.1 starts with one new migration: upgrade + pre-migration backup.
    const KNOWN_011: &[&str] = &["core_0001_meta", "storage_0001_projects", "updater_0001_new"];
    let r = prepare_start(&req(data, v011s, KNOWN_011)).unwrap();
    assert_eq!(r.start, StartKind::Upgraded { from: "0.1.0".into() });
    assert_eq!(r.schema, SchemaState::Compatible { pending: 1 });
    let b = r.backup.expect("pre-migration backup");
    assert_eq!(b.reason, BackupReason::PreMigration);
    assert_eq!((b.from_version.as_str(), b.to_version.as_deref()), ("0.1.0", Some("0.1.1")));
    let m = read_marker(data).unwrap();
    assert_eq!((m.version.as_str(), m.previous_version.as_deref()), ("0.1.1", Some("0.1.0")));

    // 0.1.1 applied its migration; rolling back to 0.1.0 must be refused, data untouched.
    let c = rusqlite::Connection::open(data.join("orkestr.db")).unwrap();
    c.execute("INSERT INTO _orkestr_migrations (id, owner, checksum, down_sql, applied_at) VALUES ('updater_0001_new','t','x','x','now')", [])
        .unwrap();
    drop(c);
    let err = prepare_start(&req(data, v010s, KNOWN)).unwrap_err();
    assert!(matches!(&err, UpdateError::DatabaseNewer(by) if by.contains("0.1.1")), "{err}");
    assert_eq!(err.code(), "database_newer");
    assert_eq!(read_marker(data).unwrap().version, "0.1.1", "a refused start must not rewrite the marker");
}

/// The state a killed / crashed core leaves behind (`-wal` with committed but
/// un-checkpointed pages, a `-shm` index) plus a stale staging dir of an
/// interrupted backup: the backup must finish, include the WAL content and
/// clean up.
#[test]
fn backup_and_schema_check_survive_a_crashed_writer() {
    let live = tempfile::tempdir().unwrap();
    let conn = make_db(&live.path().join("orkestr.db"), KNOWN);
    conn.pragma_update(None, "wal_autocheckpoint", 0).unwrap();
    conn.execute("INSERT INTO marker VALUES ('in the wal')", []).unwrap();
    // "Crash": copy the files while the writer still has them open.
    let crashed = tempfile::tempdir().unwrap();
    for f in ["orkestr.db", "orkestr.db-wal", "orkestr.db-shm"] {
        std::fs::copy(live.path().join(f), crashed.path().join(f)).unwrap();
    }
    drop(conn);
    // An interrupted earlier backup left a staging dir behind.
    std::fs::create_dir_all(crashed.path().join("backups").join(".orkestr-backup-x.tmp")).unwrap();

    let started = std::time::Instant::now();
    let r = prepare_start(&StartRequest {
        data_dir: crashed.path(),
        config_dir: None,
        version: &Version::new(0, 1, 1),
        known_migrations: &["core_0001_meta", "storage_0001_projects", "new_0001"],
        retention: 5,
    })
    .unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(30));
    let b = r.backup.expect("backup taken");
    let mut z = zip::ZipArchive::new(std::fs::File::open(&b.path).unwrap()).unwrap();
    let mut bytes = Vec::new();
    z.by_name("data/orkestr.db").unwrap().read_to_end(&mut bytes).unwrap();
    let restored = crashed.path().join("restored.db");
    std::fs::write(&restored, bytes).unwrap();
    let c = rusqlite::Connection::open(&restored).unwrap();
    let n: i64 = c.query_row("SELECT COUNT(*) FROM marker WHERE v = 'in the wal'", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1, "committed WAL content is in the backup");
    let leftovers = std::fs::read_dir(crashed.path().join("backups"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with('.'))
        .count();
    assert_eq!(leftovers, 0, "stale staging dirs are cleaned up");
}
