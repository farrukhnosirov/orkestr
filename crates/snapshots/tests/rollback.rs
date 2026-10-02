//! Snapshot + rollback scenarios on real temp directories (§25, §63.J).

use std::path::Path;
use std::sync::Arc;

use orkestr_domain::{ProjectId, SnapshotReason, TaskId, TaskSnapshotPhase};
use orkestr_protocol::methods::snapshots::{RollbackAction, SnapshotChangeKind};
use orkestr_snapshots::{CreateSnapshot, ScanOptions, SnapshotError, SnapshotService, WorkspaceRollback};
use orkestr_storage::{Compression, Db, FsObjectStore};

struct Fixture {
    _data: tempfile::TempDir,
    ws: tempfile::TempDir,
    svc: SnapshotService,
    project: ProjectId,
}

async fn fixture() -> Fixture {
    let data = tempfile::tempdir().unwrap();
    let db = Db::open(data.path().join("t.db")).unwrap();
    db.migrate(orkestr_snapshots::snapshot_migrations()).await.unwrap();
    let objects = Arc::new(FsObjectStore::open(data.path().join("objects"), Compression::Zstd { level: 3 }).unwrap());
    let svc = SnapshotService::new(db, objects, None, ScanOptions::default());
    let ws = tempfile::tempdir().unwrap();
    Fixture { _data: data, ws, svc, project: ProjectId::new() }
}

fn write(root: &Path, rel: &str, content: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

fn read(root: &Path, rel: &str) -> Option<String> {
    std::fs::read_to_string(root.join(rel)).ok()
}

fn snap_req(f: &Fixture) -> CreateSnapshot {
    CreateSnapshot::manual(f.project, f.ws.path())
}

#[tokio::test]
async fn create_captures_everything_and_dedupes() {
    let f = fixture().await;
    let root = f.ws.path();
    write(root, "src/main.rs", "fn main() {}");
    write(root, "docs/ü space.md", "# hi");
    write(root, ".gitignore", "*.tmp\n");
    write(root, "scratch.tmp", "ignored");
    write(root, "node_modules/x/index.js", "ignored");
    let s1 = f.svc.create(snap_req(&f)).await.unwrap();
    assert_eq!(s1.file_count, 3);
    assert_eq!(s1.reason, SnapshotReason::Manual);
    assert!(s1.new_bytes.unwrap() > 0);
    let detail = f.svc.detail(s1.id).await.unwrap();
    let paths: Vec<_> = detail.entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(paths, vec![".gitignore", "docs/ü space.md", "src/main.rs"]);

    let s2 = f.svc.create(snap_req(&f)).await.unwrap();
    assert_eq!(s2.new_bytes, Some(0), "unchanged content is deduplicated");
    assert_eq!(s1.workspace_id, s2.workspace_id, "same directory → same workspace id");
    let list = f.svc.list(f.project, None, None, 10).await.unwrap();
    assert_eq!(list.iter().map(|s| s.id).collect::<Vec<_>>(), vec![s2.id, s1.id], "newest first");
}

#[tokio::test]
async fn diff_reports_changes_and_contents() {
    let f = fixture().await;
    let root = f.ws.path();
    write(root, "a.txt", "one\n");
    write(root, "b.txt", "keep\n");
    write(root, "c.txt", "gone\n");
    let s = f.svc.create(snap_req(&f)).await.unwrap();
    write(root, "a.txt", "two\n");
    std::fs::remove_file(root.join("c.txt")).unwrap();
    write(root, "new/d.txt", "new\n");
    let d = f.svc.diff(s.id, Some("a.txt")).await.unwrap();
    let kinds: Vec<_> = d.changes.iter().map(|c| (c.path.as_str(), c.kind)).collect();
    assert_eq!(
        kinds,
        vec![
            ("a.txt", SnapshotChangeKind::Modified),
            ("c.txt", SnapshotChangeKind::Deleted),
            ("new/d.txt", SnapshotChangeKind::Added)
        ]
    );
    let file = d.file.unwrap();
    assert_eq!(file.snapshot.as_deref(), Some("one\n"));
    assert_eq!(file.current.as_deref(), Some("two\n"));
    assert!(!file.binary);
}

#[tokio::test]
async fn workspace_rollback_plan_apply_and_undo() {
    let f = fixture().await;
    let root = f.ws.path();
    write(root, "a.txt", "original a\n");
    write(root, "dir/b.txt", "original b\n");
    write(root, "same.txt", "same\n");
    let s = f.svc.create(snap_req(&f)).await.unwrap();

    write(root, "a.txt", "changed a\n");
    std::fs::remove_file(root.join("dir/b.txt")).unwrap();
    write(root, "created.txt", "created later\n");

    let plan = f.svc.plan_rollback(s.id, false).await.unwrap();
    let actions: Vec<_> = plan.items.iter().map(|i| (i.path.as_str(), i.action)).collect();
    assert_eq!(
        actions,
        vec![
            ("a.txt", RollbackAction::Restore),
            ("created.txt", RollbackAction::Keep),
            ("dir/b.txt", RollbackAction::Recreate)
        ]
    );
    assert_eq!(read(root, "a.txt").as_deref(), Some("changed a\n"), "planning changes nothing");

    // A stale fingerprint is refused.
    let stale = WorkspaceRollback { expected_fingerprint: Some("nope".into()), ..WorkspaceRollback::default() };
    assert!(matches!(f.svc.rollback_workspace(s.id, stale).await, Err(SnapshotError::Stale(_))));

    let res = f
        .svc
        .rollback_workspace(
            s.id,
            WorkspaceRollback { expected_fingerprint: Some(plan.fingerprint.clone()), ..WorkspaceRollback::default() },
        )
        .await
        .unwrap();
    assert_eq!(res.restored, vec!["a.txt", "dir/b.txt"]);
    assert!(res.deleted.is_empty());
    assert!(res.conflicts.is_empty());
    assert_eq!(read(root, "a.txt").as_deref(), Some("original a\n"));
    assert_eq!(read(root, "dir/b.txt").as_deref(), Some("original b\n"));
    assert_eq!(read(root, "created.txt").as_deref(), Some("created later\n"), "created files kept by default");

    // The safety snapshot can undo the rollback.
    let safety = res.pre_rollback_snapshot_id.expect("safety snapshot");
    assert_eq!(f.svc.get(safety).await.unwrap().reason, SnapshotReason::PreRollback);
    let undo = f
        .svc
        .rollback_workspace(safety, WorkspaceRollback { delete_created: true, ..Default::default() })
        .await
        .unwrap();
    assert!(undo.conflicts.is_empty());
    assert_eq!(read(root, "a.txt").as_deref(), Some("changed a\n"));
    assert!(read(root, "dir/b.txt").is_none(), "file deleted again (it did not exist in the safety snapshot)");
    assert!(!root.join("dir").exists(), "emptied directory pruned");

    // delete_created removes files created after the snapshot.
    let res =
        f.svc.rollback_workspace(s.id, WorkspaceRollback { delete_created: true, ..Default::default() }).await.unwrap();
    assert_eq!(res.deleted, vec!["created.txt"]);
    assert!(read(root, "created.txt").is_none());
    assert_eq!(read(root, "same.txt").as_deref(), Some("same\n"));
}

#[tokio::test]
async fn workspace_rollback_subset_of_paths() {
    let f = fixture().await;
    let root = f.ws.path();
    write(root, "a.txt", "a\n");
    write(root, "b.txt", "b\n");
    let s = f.svc.create(snap_req(&f)).await.unwrap();
    write(root, "a.txt", "a2\n");
    write(root, "b.txt", "b2\n");
    let res = f
        .svc
        .rollback_workspace(s.id, WorkspaceRollback { paths: Some(vec!["b.txt".into()]), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(res.restored, vec!["b.txt"]);
    assert_eq!(read(root, "a.txt").as_deref(), Some("a2\n"), "unselected path untouched");
    assert_eq!(read(root, "b.txt").as_deref(), Some("b\n"));
}

#[tokio::test]
async fn restore_single_file() {
    let f = fixture().await;
    let root = f.ws.path();
    write(root, "x/y.txt", "v1\n");
    let s = f.svc.create(snap_req(&f)).await.unwrap();
    write(root, "x/y.txt", "v2\n");
    let res = f.svc.restore_file(s.id, "x/y.txt").await.unwrap();
    assert_eq!(res.restored, vec!["x/y.txt"]);
    assert!(res.pre_rollback_snapshot_id.is_some());
    assert_eq!(read(root, "x/y.txt").as_deref(), Some("v1\n"));
    let again = f.svc.restore_file(s.id, "x/y.txt").await.unwrap();
    assert_eq!(again.unchanged, vec!["x/y.txt"]);
    assert!(again.pre_rollback_snapshot_id.is_none());
    assert!(matches!(f.svc.restore_file(s.id, "nope.txt").await, Err(SnapshotError::NotFound(_))));
    assert!(f.svc.restore_file(s.id, "../escape").await.is_err());
}

async fn task_checkpoint(f: &Fixture, task: TaskId, phase: TaskSnapshotPhase) -> orkestr_domain::WorkspaceSnapshot {
    let mut req = snap_req(f);
    req.reason = SnapshotReason::TaskCheckpoint;
    req.task_id = Some(task);
    req.task_phase = Some(phase);
    f.svc.create(req).await.unwrap()
}

#[tokio::test]
async fn task_rollback_reverts_only_task_changes_and_preserves_unrelated_work() {
    let f = fixture().await;
    let root = f.ws.path();
    write(root, "task_edit.txt", "before\n");
    write(root, "task_delete.txt", "will be deleted\n");
    write(root, "shared.txt", "base\n");
    write(root, "unrelated.txt", "user base\n");
    let task = TaskId::new();
    task_checkpoint(&f, task, TaskSnapshotPhase::Before).await;

    // The task's work.
    write(root, "task_edit.txt", "after (task)\n");
    std::fs::remove_file(root.join("task_delete.txt")).unwrap();
    write(root, "task_new.txt", "created by task\n");
    write(root, "shared.txt", "task version\n");
    task_checkpoint(&f, task, TaskSnapshotPhase::After).await;

    // Someone else works afterwards.
    write(root, "unrelated.txt", "user edit after task\n");
    write(root, "user_new.txt", "user file\n");
    write(root, "shared.txt", "user changed it again\n");

    // Dry run changes nothing.
    let dry = f.svc.rollback_task(f.project, task, None, None, true).await.unwrap();
    assert!(dry.dry_run);
    assert_eq!(dry.restored, vec!["task_delete.txt", "task_edit.txt"]);
    assert_eq!(dry.deleted, vec!["task_new.txt"]);
    assert_eq!(read(root, "task_edit.txt").as_deref(), Some("after (task)\n"));

    let res = f.svc.rollback_task(f.project, task, None, None, false).await.unwrap();
    assert!(res.pre_rollback_snapshot_id.is_some());
    assert_eq!(res.restored, vec!["task_delete.txt", "task_edit.txt"]);
    assert_eq!(res.deleted, vec!["task_new.txt"]);
    assert_eq!(res.conflicts.len(), 1);
    assert_eq!(res.conflicts[0].path, "shared.txt");

    assert_eq!(read(root, "task_edit.txt").as_deref(), Some("before\n"));
    assert_eq!(read(root, "task_delete.txt").as_deref(), Some("will be deleted\n"));
    assert!(read(root, "task_new.txt").is_none());
    // Never destroy unrelated changes.
    assert_eq!(read(root, "shared.txt").as_deref(), Some("user changed it again\n"));
    assert_eq!(read(root, "unrelated.txt").as_deref(), Some("user edit after task\n"));
    assert_eq!(read(root, "user_new.txt").as_deref(), Some("user file\n"));

    // Second rollback: everything already reverted (shared still conflicts).
    let again = f.svc.rollback_task(f.project, task, None, None, false).await.unwrap();
    assert!(again.restored.is_empty() && again.deleted.is_empty());
    assert_eq!(again.unchanged, vec!["task_delete.txt", "task_edit.txt", "task_new.txt"]);
    assert!(again.pre_rollback_snapshot_id.is_none());
}

#[tokio::test]
async fn task_rollback_requires_a_pair() {
    let f = fixture().await;
    write(f.ws.path(), "a.txt", "a");
    let task = TaskId::new();
    let err = f.svc.rollback_task(f.project, task, None, None, false).await.unwrap_err();
    assert!(matches!(err, SnapshotError::NotFound(_)));
    task_checkpoint(&f, task, TaskSnapshotPhase::Before).await;
    let err = f.svc.rollback_task(f.project, task, None, None, false).await.unwrap_err();
    assert!(matches!(err, SnapshotError::NotFound(_)), "{err}");
}

#[tokio::test]
async fn delete_snapshot() {
    let f = fixture().await;
    write(f.ws.path(), "a.txt", "a");
    let s = f.svc.create(snap_req(&f)).await.unwrap();
    f.svc.delete(s.id).await.unwrap();
    assert!(matches!(f.svc.get(s.id).await, Err(SnapshotError::NotFound(_))));
    assert!(matches!(f.svc.delete(s.id).await, Err(SnapshotError::NotFound(_))));
}
