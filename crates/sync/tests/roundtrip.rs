//! Two engines + an in-process sync server (ADR-0031 acceptance at crate level).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use orkestr_domain::{PrivacyPolicy, ProjectId, SyncPolicy};
use orkestr_sync::crypto::{generate_remote_key, DeviceKey, WorkspaceKeys};
use orkestr_sync::manifest::SignedRef;
use orkestr_sync::remote::{CasOutcome, HttpRemote};
use orkestr_sync::vv::VersionVector;
use orkestr_sync::{
    check_policy, server_router, DefaultSyncEngine, RemoteSyncConfig, Resolution, SyncEngine, SyncError,
    SyncServerState,
};

const TOKEN: &str = "tok-123";

async fn start_server(dir: &Path, listener: Option<tokio::net::TcpListener>) -> String {
    let auth: orkestr_sync::server::AuthFn = Arc::new(|t: &str| (t == TOKEN).then(|| "acct-1".to_owned()));
    let state = SyncServerState::open(dir, auth).unwrap();
    let app = axum::Router::new().nest("/v1/sync", server_router(state));
    let listener = match listener {
        Some(l) => l,
        None => tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(),
    };
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

struct Device {
    engine: DefaultSyncEngine,
    project: ProjectId,
    root: PathBuf,
}

async fn device(tmp: &Path, name: &str, url: &str, key: &str) -> Device {
    let root = tmp.join(name).join("work");
    std::fs::create_dir_all(&root).unwrap();
    let engine = DefaultSyncEngine::open(&tmp.join(name).join("sync")).unwrap();
    let project = ProjectId::new();
    engine
        .configure(
            project,
            root.clone(),
            RemoteSyncConfig {
                server_url: url.to_owned(),
                access_token: TOKEN.to_owned(),
                remote_key: key.to_owned(),
                project_key: Some("demo".into()),
            },
        )
        .await
        .unwrap();
    Device { engine, project, root }
}

fn git(root: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com", "-c", "core.autocrlf=false"])
        .args(args)
        .output()
        .expect("git available");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn write(root: &Path, rel: &str, content: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

fn read(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap()
}

#[tokio::test]
async fn round_trip_with_uncommitted_files_records_and_git_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let url = start_server(&tmp.path().join("server"), None).await;
    let key = generate_remote_key();
    let a = device(tmp.path(), "a", &url, &key).await;
    let b = device(tmp.path(), "b", &url, &key).await;

    // A: a git repo with a commit, then uncommitted edits + a new file.
    git(&a.root, &["init", "-q"]);
    write(&a.root, "src/lib.rs", "pub fn a() {}\n");
    write(&a.root, ".gitignore", "build/\n");
    git(&a.root, &["add", "."]);
    git(&a.root, &["commit", "-q", "-m", "init"]);
    let revs = git(&a.root, &["rev-list", "--count", "HEAD"]);
    write(&a.root, "src/lib.rs", "pub fn a() { /* uncommitted SECRET-MARKER */ }\n");
    write(&a.root, "notes/new.md", "new file\n");
    write(&a.root, "build/out.bin", "ignored");
    write(&a.root, ".env", "TOKEN=1");
    let big: String = (0..200_000).map(|i| char::from(b'a' + u8::try_from(i % 23).unwrap())).collect();
    write(&a.root, "data/big.txt", &big);
    a.engine.put_record(a.project, "uiState", serde_json::json!({"openEditors": ["src/lib.rs"]})).await.unwrap();

    let rep = a.engine.sync_now(a.project).await.unwrap();
    assert!(rep.pushed > 0);
    assert_eq!(git(&a.root, &["rev-list", "--count", "HEAD"]), revs, "sync never commits");

    let rep = b.engine.sync_now(b.project).await.unwrap();
    assert!(rep.pulled >= 3, "{rep:?}");
    for f in ["src/lib.rs", "notes/new.md", ".gitignore", "data/big.txt"] {
        assert_eq!(blake3::hash(read(&b.root, f).as_bytes()), blake3::hash(read(&a.root, f).as_bytes()), "{f}");
    }
    assert!(!b.root.join(".git").exists(), ".git is never synced");
    assert!(!b.root.join("build/out.bin").exists(), "gitignored files are not synced");
    assert!(!b.root.join(".env").exists(), "secret files are not synced");
    assert_eq!(
        b.engine.get_record(b.project, "uiState").await.unwrap(),
        Some(serde_json::json!({"openEditors": ["src/lib.rs"]}))
    );
    let st = b.engine.status(b.project).await.unwrap();
    assert!(st.configured && st.last_sync_at.is_some() && st.pending == 0 && st.last_error.is_none());

    // The server holds ciphertext only.
    for f in walk(&tmp.path().join("server")) {
        let bytes = std::fs::read(&f).unwrap();
        assert!(!contains(&bytes, b"SECRET-MARKER"), "plaintext leaked into {}", f.display());
        assert!(!contains(&bytes, b"openEditors"), "record leaked into {}", f.display());
        assert!(!contains(&bytes, b"notes/new.md"), "path leaked into {}", f.display());
    }

    // Second round is a no-op; edits flow back from B to A.
    let rep = b.engine.sync_now(b.project).await.unwrap();
    assert_eq!(rep.pulled, 0);
    write(&b.root, "notes/new.md", "edited on B\n");
    b.engine.sync_now(b.project).await.unwrap();
    a.engine.sync_now(a.project).await.unwrap();
    assert_eq!(read(&a.root, "notes/new.md"), "edited on B\n");
    assert_eq!(git(&a.root, &["rev-list", "--count", "HEAD"]), revs);
}

#[tokio::test]
async fn first_sync_of_a_clone_takes_remote_edits_without_conflicts() {
    let tmp = tempfile::tempdir().unwrap();
    let url = start_server(&tmp.path().join("server"), None).await;
    let key = generate_remote_key();
    let a = device(tmp.path(), "a", &url, &key).await;
    let b = device(tmp.path(), "b", &url, &key).await;
    git(&a.root, &["init", "-q"]);
    write(&a.root, "f.txt", "base\n");
    git(&a.root, &["add", "."]);
    git(&a.root, &["commit", "-q", "-m", "init"]);
    let clone = b.root.to_string_lossy().to_string();
    std::fs::remove_dir_all(&b.root).unwrap();
    Command::new("git").args(["-c", "core.autocrlf=false", "clone", "-q"]).arg(&a.root).arg(&clone).status().unwrap();
    write(&a.root, "f.txt", "edited on A\n");
    a.engine.sync_now(a.project).await.unwrap();
    let rep = b.engine.sync_now(b.project).await.unwrap();
    assert_eq!(rep.conflicts, 0);
    assert_eq!(read(&b.root, "f.txt"), "edited on A\n");
    assert!(git(&b.root, &["status", "--porcelain"]).contains("f.txt"), "uncommitted on B");
}

#[tokio::test]
async fn concurrent_edit_keeps_both_versions() {
    let tmp = tempfile::tempdir().unwrap();
    let url = start_server(&tmp.path().join("server"), None).await;
    let key = generate_remote_key();
    let a = device(tmp.path(), "a", &url, &key).await;
    let b = device(tmp.path(), "b", &url, &key).await;
    write(&a.root, "doc.txt", "v0\n");
    a.engine.sync_now(a.project).await.unwrap();
    b.engine.sync_now(b.project).await.unwrap();

    write(&a.root, "doc.txt", "from A\n");
    write(&b.root, "doc.txt", "from B\n");
    a.engine.sync_now(a.project).await.unwrap();
    let rep = b.engine.sync_now(b.project).await.unwrap();
    assert_eq!(rep.conflicts, 1);
    let conflicts = b.engine.list_conflicts(b.project).await.unwrap();
    assert_eq!(conflicts.len(), 1);
    let c = &conflicts[0];
    assert_eq!(c.path, "doc.txt");
    let copy = c.conflict_copy.clone().unwrap();
    assert_eq!(read(&b.root, "doc.txt"), "from B\n");
    assert_eq!(read(&b.root, &copy), "from A\n");
    assert_eq!(b.engine.status(b.project).await.unwrap().conflicts, 1);

    // A receives B's version and the copy: nothing lost anywhere.
    a.engine.sync_now(a.project).await.unwrap();
    assert_eq!(read(&a.root, "doc.txt"), "from B\n");
    assert_eq!(read(&a.root, &copy), "from A\n");

    b.engine.resolve_conflict(c.id.clone(), Resolution::KeepRemote).await.unwrap();
    assert_eq!(read(&b.root, "doc.txt"), "from A\n");
    assert!(!b.root.join(&copy).exists());
    assert!(b.engine.list_conflicts(b.project).await.unwrap().is_empty());
    b.engine.sync_now(b.project).await.unwrap();
    a.engine.sync_now(a.project).await.unwrap();
    assert_eq!(read(&a.root, "doc.txt"), "from A\n");
    assert!(!a.root.join(&copy).exists());
}

#[tokio::test]
async fn tampered_chunk_is_an_integrity_error() {
    let tmp = tempfile::tempdir().unwrap();
    let server_dir = tmp.path().join("server");
    let url = start_server(&server_dir, None).await;
    let key = generate_remote_key();
    let a = device(tmp.path(), "a", &url, &key).await;
    let b = device(tmp.path(), "b", &url, &key).await;
    write(&a.root, "x.txt", "payload");
    a.engine.sync_now(a.project).await.unwrap();
    for f in walk(&server_dir) {
        if f.components().any(|c| c.as_os_str() == "chunks") {
            let mut bytes = std::fs::read(&f).unwrap();
            bytes[30] ^= 0x55;
            std::fs::write(&f, bytes).unwrap();
        }
    }
    let err = b.engine.sync_now(b.project).await.unwrap_err();
    assert!(matches!(err, SyncError::Integrity(_)), "{err:?}");
    assert!(!b.root.join("x.txt").exists());
    assert!(b.engine.status(b.project).await.unwrap().last_error.is_some());
}

#[tokio::test]
async fn rollback_and_unknown_signer_are_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let url = start_server(&tmp.path().join("server"), None).await;
    let key = generate_remote_key();
    let a = device(tmp.path(), "a", &url, &key).await;
    let b = device(tmp.path(), "b", &url, &key).await;
    let http = HttpRemote::new(&url, TOKEN).unwrap();
    let sid = WorkspaceKeys::from_remote_key(&key).unwrap().stream_id("workspace:demo");

    write(&a.root, "f", "1");
    a.engine.sync_now(a.project).await.unwrap();
    let old = http.get_head(&sid).await.unwrap().unwrap();
    write(&a.root, "f", "2");
    a.engine.sync_now(a.project).await.unwrap();
    b.engine.sync_now(b.project).await.unwrap();
    assert_eq!(read(&b.root, "f"), "2");

    // A malicious server replays the old (validly signed) head.
    let cur = http.get_head(&sid).await.unwrap().unwrap();
    assert!(matches!(http.cas_head(&sid, Some(&cur.etag), old.value.clone()).await.unwrap(), CasOutcome::Updated(_)));
    let err = b.engine.sync_now(b.project).await.unwrap_err();
    assert!(matches!(&err, SyncError::Integrity(m) if m.contains("rollback")), "{err:?}");

    // A head signed by a device outside the key ring.
    let cur = http.get_head(&sid).await.unwrap().unwrap();
    let real: SignedRef = serde_json::from_str(&cur.value).unwrap();
    let mut vv = VersionVector::default();
    vv.merge(&real.version);
    vv.bump("d_evil");
    vv.bump("d_evil");
    let forged = SignedRef::sign(&DeviceKey::generate(), "d_evil", &sid, &real.manifest_id, vv);
    http.cas_head(&sid, Some(&cur.etag), serde_json::to_string(&forged).unwrap()).await.unwrap();
    let err = b.engine.sync_now(b.project).await.unwrap_err();
    assert!(matches!(&err, SyncError::Integrity(m) if m.contains("unknown signer")), "{err:?}");
}

#[tokio::test]
async fn offline_outbox_flushes_when_the_server_returns() {
    let tmp = tempfile::tempdir().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let url = format!("http://{addr}");
    let key = generate_remote_key();
    let a = device(tmp.path(), "a", &url, &key).await;
    write(&a.root, "offline.txt", "written while offline");
    let err = a.engine.sync_now(a.project).await.unwrap_err();
    assert!(matches!(err, SyncError::Network(_)), "{err:?}");
    let st = a.engine.status(a.project).await.unwrap();
    assert!(st.pending > 0 && st.last_error.is_some());

    let l = tokio::net::TcpListener::bind(addr).await.unwrap();
    start_server(&tmp.path().join("server"), Some(l)).await;
    a.engine.sync_now(a.project).await.unwrap();
    let st = a.engine.status(a.project).await.unwrap();
    assert_eq!((st.pending, st.last_error), (0, None));
    let b = device(tmp.path(), "b", &url, &key).await;
    b.engine.sync_now(b.project).await.unwrap();
    assert_eq!(read(&b.root, "offline.txt"), "written while offline");
}

#[tokio::test]
async fn policy_and_configuration_guards() {
    let deny = PrivacyPolicy { sync: SyncPolicy::NoRemoteSync, ..PrivacyPolicy::default() };
    assert!(matches!(check_policy(&deny), Err(SyncError::PolicyDenied(_))));
    check_policy(&PrivacyPolicy::default()).unwrap();

    let tmp = tempfile::tempdir().unwrap();
    let e = DefaultSyncEngine::open(tmp.path()).unwrap();
    let p = ProjectId::new();
    assert!(matches!(e.sync_now(p).await, Err(SyncError::NotConfigured)));
    assert!(!e.status(p).await.unwrap().configured);
    let bad = RemoteSyncConfig {
        server_url: "ftp://x".into(),
        access_token: "t".into(),
        remote_key: generate_remote_key(),
        project_key: None,
    };
    assert!(matches!(e.configure(p, tmp.path().to_path_buf(), bad).await, Err(SyncError::Config(_))));
    // Device identity is stable across reopen.
    let id = e.device_id().to_owned();
    drop(e);
    assert_eq!(DefaultSyncEngine::open(tmp.path()).unwrap().device_id(), id);
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_none_or(|x| x != "db" && x != "db-wal" && x != "db-shm") {
                out.push(p);
            }
        }
    }
    out
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}
