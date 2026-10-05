//! `ArchitectureService` on fixture repositories: first scan, incremental
//! re-scan, snapshots, diffs, pruning-safe history and the debounced
//! auto-refresh after `FileChanged` events.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use orkestr_domain::{now, Project, ProjectId};
use orkestr_events::{EventBus, Received};
use orkestr_insight::arch::{spawn_auto_refresh, AutoRefreshConfig};
use orkestr_insight::{insight_migrations, ArchitectureService, ScanOptions};
use orkestr_protocol::event::{EventKind, FileChangeActor, FileChangeKind, FileChangedPayload, NewEvent};
use orkestr_protocol::methods::architecture::{ArchEdgeKind, ArchNodeKind};
use orkestr_storage::{Db, ProjectRepository, SqliteMetadataStore};
use tokio_util::sync::CancellationToken;

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

async fn db(dir: &Path) -> Db {
    let db = Db::open(dir.join("t.db")).unwrap();
    let mut migrations = orkestr_storage::storage_migrations();
    migrations.extend(insight_migrations());
    db.migrate(migrations).await.unwrap();
    db
}

/// npm workspaces + a Python service + Go module + express routes.
fn polyglot(root: &Path) {
    write(root, "package.json", r#"{"name":"mono","private":true,"workspaces":["packages/*"]}"#);
    write(root, "packages/core/package.json", r#"{"name":"@mono/core","version":"1.0.0","dependencies":{"lodash":"^4"}}"#);
    write(root, "packages/core/src/index.ts", "export const core = 1;\n");
    write(
        root,
        "packages/server/package.json",
        r#"{"name":"@mono/server","version":"1.0.0","dependencies":{"@mono/core":"workspace:*","express":"^4"}}"#,
    );
    write(
        root,
        "packages/server/src/app.ts",
        "import express from 'express';\nimport { core } from '@mono/core';\nconst app = express();\napp.get('/api/items', h);\napp.post('/api/items', h);\n",
    );
    write(root, "worker/pyproject.toml", "[project]\nname = \"worker\"\ndependencies = [\"fastapi\", \"sqlalchemy\"]\n");
    write(
        root,
        "worker/worker/main.py",
        "from fastapi import FastAPI\napp = FastAPI()\n\n@app.get(\"/jobs\")\ndef jobs():\n    return []\n",
    );
    write(root, "worker/Dockerfile", "FROM python:3.12\nCMD [\"python\", \"-m\", \"worker\"]\n");
    write(root, "tools/go.mod", "module example.com/tools\n\ngo 1.22\n");
    write(root, "tools/main.go", "package main\n\nfunc main() {}\n");
    write(root, "node_modules/junk/package.json", r#"{"name":"junk"}"#);
}

#[tokio::test]
async fn polyglot_fixture_scan_incremental_and_diff() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    polyglot(&root);
    let svc = ArchitectureService::new(db(dir.path()).await, None, ScanOptions::default());
    let pid = ProjectId::new();

    let first = svc.refresh(pid, root.clone(), false, false).await.unwrap();
    assert!(first.changed);
    assert!(first.diff.is_none(), "no previous snapshot");
    let map = &first.map;
    let names: Vec<&str> = map.nodes.iter().map(|n| n.name.as_str()).collect();
    assert!(names.contains(&"@mono/core"), "{names:?}");
    assert!(names.contains(&"@mono/server"), "{names:?}");
    assert!(names.contains(&"worker"), "{names:?}");
    assert!(!names.contains(&"junk"), "node_modules ignored: {names:?}");
    assert!(map.nodes.iter().any(|n| n.ecosystem == "go"), "{names:?}");
    assert!(map.nodes.iter().any(|n| n.kind == ArchNodeKind::Service), "Dockerfile service");
    let routes: Vec<String> =
        map.nodes.iter().flat_map(|n| n.routes.iter().map(|r| format!("{} {}", r.method, r.path))).collect();
    assert!(routes.contains(&"GET /api/items".to_owned()), "{routes:?}");
    assert!(routes.contains(&"POST /api/items".to_owned()), "{routes:?}");
    assert!(routes.contains(&"GET /jobs".to_owned()), "{routes:?}");
    let server = map.nodes.iter().find(|n| n.name == "@mono/server").unwrap();
    let core = map.nodes.iter().find(|n| n.name == "@mono/core").unwrap();
    assert!(
        map.edges.iter().any(|e| e.from == server.id && e.to == core.id && e.kind == ArchEdgeKind::DependsOn),
        "workspace dependency edge: {:?}",
        map.edges
    );
    assert!(map.nodes.iter().any(|n| n.kind == ArchNodeKind::External && n.dependencies.iter().any(|d| d == "express")));
    let parsed_first = map.stats.files_parsed;
    assert!(parsed_first > 0);

    // Unchanged incremental scan: nothing re-parsed, no new snapshot.
    let same = svc.refresh(pid, root.clone(), false, false).await.unwrap();
    assert!(!same.changed);
    assert_eq!(same.map.snapshot_id, map.snapshot_id);

    // Change one file: only it is parsed; the route shows up in the diff.
    std::thread::sleep(Duration::from_millis(20));
    write(
        &root,
        "packages/server/src/app.ts",
        "import express from 'express';\nimport { core } from '@mono/core';\nconst app = express();\napp.get('/api/items', h);\napp.post('/api/items', h);\napp.delete('/api/items/:id', h);\n",
    );
    let changed = svc.refresh(pid, root.clone(), false, false).await.unwrap();
    assert!(changed.changed);
    assert!(changed.map.stats.files_parsed < parsed_first, "incremental: {} < {parsed_first}", changed.map.stats.files_parsed);
    let diff = changed.diff.unwrap();
    assert!(diff.added_nodes.is_empty() && diff.removed_nodes.is_empty(), "{diff:?}");
    assert!(
        diff.changed_nodes.iter().any(|c| c.added_routes.iter().any(|r| r.method == "DELETE" && r.path == "/api/items/:id")),
        "{diff:?}"
    );

    // Remove the Go module → removed node; history and explicit diffs.
    std::fs::remove_dir_all(root.join("tools")).unwrap();
    let removed = svc.refresh(pid, root.clone(), false, false).await.unwrap();
    assert!(removed.diff.as_ref().unwrap().removed_nodes.iter().any(|n| n.ecosystem == "go"));
    let history = svc.snapshots(pid).await.unwrap();
    assert_eq!(history.len(), 3);
    assert_eq!(history[0].snapshot_id, removed.map.snapshot_id, "newest first");
    let whole = svc.diff(pid, Some(&history[2].snapshot_id), None).await.unwrap();
    assert!(whole.removed_nodes.iter().any(|n| n.ecosystem == "go"));
    assert!(!whole.changed_nodes.is_empty());
    let stored = svc.get(pid, Some(&history[2].snapshot_id)).await.unwrap().unwrap();
    assert_eq!(stored, first.map);
    assert!(svc.get(pid, Some("missing")).await.is_err());
    assert!(svc.diff(ProjectId::new(), None, None).await.is_err(), "no snapshots → not found");

    // A full re-scan of an unchanged tree yields the same digest.
    let full = svc.refresh(pid, root, true, false).await.unwrap();
    assert!(!full.changed);
}

#[tokio::test]
async fn file_changes_trigger_a_debounced_auto_refresh() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    polyglot(&root);
    let db = db(dir.path()).await;
    let projects = Arc::new(SqliteMetadataStore::new(db.clone()));
    let mut project = Project::new("Auto", now());
    project.root_path = Some(root.to_string_lossy().into_owned());
    projects.insert(&project).await.unwrap();
    let never_opened = Project::new("Never", now());
    projects.insert(&never_opened).await.unwrap();
    let pid = project.id;

    let bus = EventBus::in_memory(256);
    let svc = ArchitectureService::new(db, Some(bus.clone()), ScanOptions::default());
    let shutdown = CancellationToken::new();
    let cfg = AutoRefreshConfig { debounce: Duration::from_millis(150), min_interval: Duration::from_millis(0) };
    let handle = spawn_auto_refresh(svc.clone(), bus.clone(), projects as Arc<dyn ProjectRepository>, cfg, shutdown.clone());
    let mut sub = bus.subscribe_all();

    // Opened once (snapshot exists) → eligible for auto refresh.
    svc.refresh(pid, root.clone(), false, false).await.unwrap();

    write(&root, "packages/extra/package.json", r#"{"name":"@mono/extra","version":"1.0.0"}"#);
    let file_changed = |path: &str| {
        EventKind::FileChanged(FileChangedPayload {
            path: path.into(),
            old_path: None,
            change: FileChangeKind::Created,
            actor: FileChangeActor::default(),
            agent_id: None,
        })
    };
    // A burst of changes collapses into one refresh.
    for _ in 0..5 {
        bus.publish(NewEvent::new(file_changed("packages/extra/package.json")).project(pid)).await.unwrap();
    }
    // Events for a project without a snapshot are ignored.
    bus.publish(NewEvent::new(file_changed("x.txt")).project(never_opened.id)).await.unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut automatic_updates = 0;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(800), sub.recv()).await {
            Ok(Some(Received::Event(ev))) => {
                if let EventKind::ArchitectureMapUpdated(p) = &ev.kind {
                    if p.automatic {
                        assert_eq!(p.project_id, pid);
                        assert_eq!(p.added_nodes, 1, "{p:?}");
                        automatic_updates += 1;
                    }
                }
            }
            Ok(_) => {}
            Err(_) if automatic_updates > 0 => break,
            Err(_) => {}
        }
    }
    assert_eq!(automatic_updates, 1, "debounced into exactly one automatic refresh");
    let latest = svc.get(pid, None).await.unwrap().unwrap();
    assert!(latest.nodes.iter().any(|n| n.name == "@mono/extra"));
    assert!(svc.get(never_opened.id, None).await.unwrap().is_none(), "never scanned automatically");

    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(5), handle).await.unwrap().unwrap();
}
