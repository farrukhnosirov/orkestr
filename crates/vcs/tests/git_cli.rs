//! Integration tests against real temp repositories (requires `git` ≥ 2.38).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use orkestr_domain::{AgentId, ProjectId};
use orkestr_protocol::methods::vcs::{ConflictKind, DiffTarget, FileChangeStatus, FileRevision, MergeStatus};
use orkestr_storage::Db;
use orkestr_vcs::{AddWorktreeRequest, DiffRequest, GitCli, RunOptions, Vcs, VcsError};

fn git() -> Option<Arc<GitCli>> {
    let g = GitCli::discover();
    if g.is_available() {
        Some(Arc::new(g))
    } else {
        eprintln!("git not installed — skipping");
        None
    }
}

async fn sh(g: &GitCli, dir: &Path, args: &[&str]) -> String {
    g.runner().run_ok(dir, args, RunOptions::write()).await.unwrap().text()
}

fn write(root: &Path, rel: &str, content: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

async fn repo(g: &GitCli) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    g.init(dir.path(), Some("main")).await.unwrap();
    sh(g, dir.path(), &["config", "user.name", "Test"]).await;
    sh(g, dir.path(), &["config", "user.email", "test@example.invalid"]).await;
    sh(g, dir.path(), &["config", "core.autocrlf", "false"]).await;
    write(dir.path(), "README.md", "hello\n");
    write(dir.path(), "f.txt", "a\nb\nc\n");
    g.stage(dir.path(), &[], true).await.unwrap();
    g.commit(dir.path(), "init", false, &[]).await.unwrap();
    dir
}

fn diff_req(target: DiffTarget) -> DiffRequest {
    DiffRequest { target, paths: vec![], context_lines: 3, include_untracked: false, max_patch_bytes: 512 * 1024 }
}

#[tokio::test]
async fn version_and_non_repo_status() {
    let Some(g) = git() else { return };
    let v = g.version().await.unwrap();
    assert!(v.major >= 2);
    let dir = tempfile::tempdir().unwrap();
    let st = g.status(dir.path()).await.unwrap();
    assert!(!st.is_repo);
    assert!(st.git_version.is_some());
    assert!(g.repo_info(dir.path()).await.unwrap().is_none());
}

#[tokio::test]
async fn status_stage_commit_with_unicode_spaces_and_renames() {
    let Some(g) = git() else { return };
    let r = repo(&g).await;
    let root = r.path();
    write(root, "dir with space/ünïcode file.txt", "x\n");
    write(root, "f.txt", "a\nB\nc\n");
    sh(&g, root, &["mv", "README.md", "READ ME.md"]).await;

    let st = g.status(root).await.unwrap();
    assert!(st.is_repo);
    assert_eq!(st.branch.as_deref(), Some("main"));
    assert!(!st.clean);
    let find =
        |p: &str| st.entries.iter().find(|e| e.path == p).cloned().unwrap_or_else(|| panic!("{p} in {:?}", st.entries));
    assert!(find("dir with space/ünïcode file.txt").is_untracked());
    assert!(find("f.txt").is_unstaged());
    let renamed = find("READ ME.md");
    assert_eq!(renamed.index, FileChangeStatus::Renamed);
    assert_eq!(renamed.orig_path.as_deref(), Some("README.md"));

    g.stage(root, &["dir with space/ünïcode file.txt".into(), "f.txt".into()], false).await.unwrap();
    let st = g.status(root).await.unwrap();
    assert!(st.entries.iter().all(|e| e.is_staged()), "{:?}", st.entries);

    g.unstage(root, &["f.txt".into()], false).await.unwrap();
    let st = g.status(root).await.unwrap();
    assert!(st.entries.iter().find(|e| e.path == "f.txt").unwrap().is_unstaged());

    let staged = g.diff(root, &diff_req(DiffTarget::Staged)).await.unwrap();
    assert!(staged
        .files
        .iter()
        .any(|f| f.path == "dir with space/ünïcode file.txt" && f.status == FileChangeStatus::Added));
    let unstaged = g.diff(root, &diff_req(DiffTarget::Unstaged)).await.unwrap();
    let f = &unstaged.files[0];
    assert_eq!((f.path.as_str(), f.additions, f.deletions), ("f.txt", 1, 1));
    assert!(f.patch.contains("-b") && f.patch.contains("+B"));

    let c = g.commit(root, "second commit\n\nbody", false, &[]).await.unwrap();
    assert_eq!(c.subject, "second commit");
    assert!(matches!(g.commit(root, "  ", false, &[]).await, Err(VcsError::InvalidArgument(_))));

    let st = g.status(root).await.unwrap();
    assert!(st.entries.iter().all(|e| e.path == "f.txt"));
    // Nothing staged → NothingToCommit.
    assert!(matches!(g.commit(root, "nothing", false, &[]).await, Err(VcsError::NothingToCommit)));

    let diff = g.diff(root, &diff_req(DiffTarget::Commit { commit: c.id.clone() })).await.unwrap();
    assert!(diff.files.iter().any(|f| f.path == "READ ME.md" && f.old_path.as_deref() == Some("README.md")));
    // Root commit diff works too.
    let log = g.log(root, None, None, 0, 10).await.unwrap();
    let root_commit = log.commits.last().unwrap();
    let d = g.diff(root, &diff_req(DiffTarget::Commit { commit: root_commit.id.clone() })).await.unwrap();
    assert_eq!(d.files.len(), 2);
}

#[tokio::test]
async fn untracked_in_diff_show_and_log_pagination() {
    let Some(g) = git() else { return };
    let r = repo(&g).await;
    let root = r.path();
    for i in 0..5 {
        write(root, "f.txt", &format!("v{i}\n"));
        g.stage(root, &[], true).await.unwrap();
        g.commit(root, &format!("c{i}"), false, &[]).await.unwrap();
    }
    let page1 = g.log(root, None, None, 0, 4).await.unwrap();
    assert_eq!(page1.commits.len(), 4);
    assert!(page1.has_more);
    let page2 = g.log(root, None, None, 4, 4).await.unwrap();
    assert_eq!(page2.commits.len(), 2);
    assert!(!page2.has_more);
    assert_eq!(page2.commits[1].subject, "init");
    let path_log = g.log(root, None, Some("README.md"), 0, 10).await.unwrap();
    assert_eq!(path_log.commits.len(), 1);

    write(root, "new.txt", "line1\nline2\n");
    let mut req = diff_req(DiffTarget::Head);
    req.include_untracked = true;
    let d = g.diff(root, &req).await.unwrap();
    let u = d.files.iter().find(|f| f.path == "new.txt").unwrap();
    assert_eq!(u.status, FileChangeStatus::Untracked);
    assert_eq!(u.additions, 2);
    assert!(u.patch.contains("+line2"));

    let head = g.show(root, "f.txt", &FileRevision::Head).await.unwrap();
    assert_eq!(head.content, "v4\n");
    let old = g.show(root, "f.txt", &FileRevision::Commit { rev: "HEAD~2".into() }).await.unwrap();
    assert_eq!(old.content, "v2\n");
    let wt = g.show(root, "new.txt", &FileRevision::Worktree).await.unwrap();
    assert!(wt.exists && wt.content.starts_with("line1"));
    let missing = g.show(root, "new.txt", &FileRevision::Head).await.unwrap();
    assert!(!missing.exists);
    assert!(g.show(root, "../x", &FileRevision::Worktree).await.is_err());
    assert!(g.log(root, Some("--output=x"), None, 0, 1).await.is_err());
}

#[tokio::test]
async fn branches_create_checkout_delete() {
    let Some(g) = git() else { return };
    let r = repo(&g).await;
    let root = r.path();
    let b = g.create_branch(root, "feature/x", None, true).await.unwrap();
    assert_eq!(b.branch, "feature/x");
    assert!(matches!(g.create_branch(root, "feature/x", None, false).await, Err(VcsError::RefChanged(_))));
    assert!(g.create_branch(root, "bad..name", None, false).await.is_err());
    write(root, "x.txt", "x\n");
    g.stage(root, &[], true).await.unwrap();
    g.commit(root, "on feature", false, &[]).await.unwrap();
    let list = g.branches(root).await.unwrap();
    assert_eq!(list.current.as_deref(), Some("feature/x"));
    assert!(list.branches.iter().any(|b| b.name == "main" && !b.current));

    g.checkout(root, "main").await.unwrap();
    // Unmerged branch: safe delete refuses, force works.
    assert!(matches!(g.delete_branch(root, "feature/x", false).await, Err(VcsError::Dirty(_))));
    let del = g.delete_branch(root, "feature/x", true).await.unwrap();
    assert_eq!(del.name, "feature/x");
    assert!(matches!(g.checkout(root, "feature/x").await, Err(VcsError::NotFound(_))));
}

#[tokio::test]
async fn discard_restores_tracked_and_deletes_untracked() {
    let Some(g) = git() else { return };
    let r = repo(&g).await;
    let root = r.path();
    write(root, "f.txt", "changed\n");
    write(root, "junk/new file.txt", "junk\n");
    write(root, "keep.txt", "keep\n");
    let (restored, deleted) = g.discard(root, &["f.txt".into(), "junk".into()]).await.unwrap();
    assert_eq!(restored, vec!["f.txt"]);
    assert_eq!(deleted, vec!["junk/new file.txt"]);
    assert_eq!(std::fs::read_to_string(root.join("f.txt")).unwrap(), "a\nb\nc\n");
    assert!(!root.join("junk/new file.txt").exists());
    assert!(root.join("keep.txt").exists(), "unselected untracked file kept");
}

async fn diverge(g: &GitCli, root: &Path, conflicting: bool) {
    g.create_branch(root, "feat", None, true).await.unwrap();
    write(root, "f.txt", "a\nFEAT\nc\n");
    write(root, "feat-only.txt", "f\n");
    g.stage(root, &[], true).await.unwrap();
    g.commit(root, "feat work", false, &[]).await.unwrap();
    g.checkout(root, "main").await.unwrap();
    if conflicting {
        write(root, "f.txt", "a\nMAIN\nc\n");
    } else {
        write(root, "main-only.txt", "m\n");
    }
    g.stage(root, &[], true).await.unwrap();
    g.commit(root, "main work", false, &[]).await.unwrap();
}

#[tokio::test]
async fn merge_preview_clean_and_merge_no_ff() {
    let Some(g) = git() else { return };
    let r = repo(&g).await;
    let root = r.path();
    diverge(&g, root, false).await;
    let p = g.merge_preview(root, "feat", None).await.unwrap();
    assert!(p.clean && !p.up_to_date);
    assert_eq!(p.target, "main");
    assert_eq!(p.commits_to_merge, 1);
    assert!(p.tree.is_some());
    let before = g.status(root).await.unwrap();
    assert!(before.clean, "preview touches no worktree");

    let m = g.merge(root, "feat", None, None, false).await.unwrap();
    assert_eq!(m.status, MergeStatus::Merged);
    assert!(!m.ref_only);
    let head = g.commit_info(root, "HEAD").await.unwrap();
    assert_eq!(head.parents.len(), 2, "--no-ff creates a merge commit");
    assert!(root.join("feat-only.txt").exists());
    let again = g.merge(root, "feat", None, None, false).await.unwrap();
    assert_eq!(again.status, MergeStatus::UpToDate);
}

#[tokio::test]
async fn merge_into_branch_not_checked_out_updates_ref_only() {
    let Some(g) = git() else { return };
    let r = repo(&g).await;
    let root = r.path();
    diverge(&g, root, false).await;
    // Merge main into feat while main is checked out: feat is not checked out anywhere.
    let m = g.merge(root, "main", Some("feat"), Some("sync main"), false).await.unwrap();
    assert_eq!(m.status, MergeStatus::Merged);
    assert!(m.ref_only);
    let c = g.commit_info(root, "feat").await.unwrap();
    assert_eq!(c.subject, "sync main");
    assert_eq!(c.parents.len(), 2);
    assert!(g.status(root).await.unwrap().clean);
}

#[tokio::test]
async fn conflict_detection_merge_and_abort() {
    let Some(g) = git() else { return };
    let r = repo(&g).await;
    let root = r.path();
    diverge(&g, root, true).await;
    let p = g.merge_preview(root, "feat", None).await.unwrap();
    assert!(!p.clean);
    assert_eq!(p.conflicts.iter().map(|c| c.path.as_str()).collect::<Vec<_>>(), vec!["f.txt"]);
    assert!(p.tree.is_none());
    assert!(g.status(root).await.unwrap().clean, "preview leaves the worktree untouched");

    // Refused without allow_conflicts.
    assert!(matches!(g.merge(root, "feat", None, None, false).await, Err(VcsError::Conflict(_))));
    // Ref-only merge with conflicts is always refused.
    assert!(matches!(g.merge(root, "main", Some("feat"), None, true).await, Err(VcsError::Conflict(_))));

    let m = g.merge(root, "feat", None, None, true).await.unwrap();
    assert_eq!(m.status, MergeStatus::Conflicted);
    assert_eq!(m.conflicts, vec!["f.txt"]);
    let st = g.status(root).await.unwrap();
    assert!(st.merge_in_progress);
    let e = st.entries.iter().find(|e| e.path == "f.txt").unwrap();
    assert_eq!(e.conflict, Some(ConflictKind::BothModified));
    assert!(matches!(g.discard(root, &["f.txt".into()]).await, Err(VcsError::Conflict(_))));

    g.abort_merge(root).await.unwrap();
    let st = g.status(root).await.unwrap();
    assert!(!st.merge_in_progress && st.clean);
    assert!(g.abort_merge(root).await.is_err());
}

fn vcs(g: &Arc<GitCli>, data: &Path) -> Vcs {
    let db = Db::open_in_memory().unwrap();
    db.call_blocking(|c| orkestr_storage::Migrator::new(orkestr_vcs::vcs_migrations()).apply(c).map(|_| ())).unwrap();
    Vcs::new(Arc::clone(g), db, data.join("worktrees"))
}

#[tokio::test]
async fn agent_worktrees_add_list_remove() {
    let Some(g) = git() else { return };
    let r = repo(&g).await;
    let root = r.path();
    let data = tempfile::tempdir().unwrap();
    let v = vcs(&g, data.path());
    let project = ProjectId::new();
    let agent = AgentId::new();
    let short = orkestr_vcs::naming::short_id(&agent.to_string());

    let w = v
        .add_worktree(
            project,
            root,
            AddWorktreeRequest { agent_id: Some(agent), role: Some("Backend".into()), ..Default::default() },
        )
        .await
        .unwrap();
    assert_eq!(w.branch.as_deref(), Some(format!("agent/backend-{short}").as_str()));
    assert!(w.managed && !w.is_main);
    let expected: PathBuf =
        data.path().join("worktrees").join(orkestr_vcs::naming::short_id(&project.to_string())).join(&short);
    assert_eq!(orkestr_vcs::naming::path_key(Path::new(&w.path)), orkestr_vcs::naming::path_key(&expected));
    assert!(expected.join("f.txt").exists());

    // Same agent again → collision suffix.
    let w2 = v
        .add_worktree(
            project,
            root,
            AddWorktreeRequest { agent_id: Some(agent), role: Some("backend".into()), ..Default::default() },
        )
        .await
        .unwrap();
    assert_eq!(w2.branch.as_deref(), Some(format!("agent/backend-{short}-2").as_str()));

    let list = v.worktrees(project, root).await.unwrap();
    assert_eq!(list.len(), 3);
    assert!(list[0].is_main && !list[0].managed);
    let listed = list.iter().find(|x| x.branch == w.branch).unwrap();
    assert_eq!(listed.agent_id, Some(agent));
    assert_eq!(listed.dirty, Some(false));

    // Resolve the worktree for requests; arbitrary directories are rejected.
    let dir = v.resolve_dir(root, Some(&w.path)).await.unwrap();
    assert!(dir.join("f.txt").exists());
    assert!(v.resolve_dir(root, Some(&data.path().to_string_lossy())).await.is_err());

    // Dirty worktree: refused without force.
    write(&dir, "wip.txt", "wip\n");
    assert!(matches!(v.remove_worktree(project, root, &w.path, false).await, Err(VcsError::Dirty(_))));
    v.remove_worktree(project, root, &w.path, true).await.unwrap();
    assert!(!dir.exists());
    v.remove_worktree(project, root, &w2.path, false).await.unwrap();
    let list = v.worktrees(project, root).await.unwrap();
    assert_eq!(list.len(), 1);
    // The main worktree can never be removed.
    assert!(v.remove_worktree(project, root, &list[0].path, true).await.is_err());
    // Branches survive removal (never deleted implicitly).
    assert!(g.rev_parse(root, &format!("refs/heads/agent/backend-{short}")).await.unwrap().is_some());
}

#[tokio::test]
async fn concurrent_worktree_creation_is_serialized() {
    let Some(g) = git() else { return };
    let r = repo(&g).await;
    let root = r.path().to_path_buf();
    let data = tempfile::tempdir().unwrap();
    let v = Arc::new(vcs(&g, data.path()));
    let project = ProjectId::new();
    let mut handles = Vec::new();
    for i in 0..8 {
        let v = Arc::clone(&v);
        let root = root.clone();
        handles.push(tokio::spawn(async move {
            v.add_worktree(
                project,
                &root,
                AddWorktreeRequest {
                    agent_id: Some(AgentId::new()),
                    role: Some(format!("qa{i}")),
                    ..Default::default()
                },
            )
            .await
        }));
    }
    for h in handles {
        h.await.unwrap().unwrap();
    }
    assert_eq!(v.worktrees(project, &root).await.unwrap().len(), 9);
}

#[tokio::test]
async fn worktree_needs_initial_commit() {
    let Some(g) = git() else { return };
    let dir = tempfile::tempdir().unwrap();
    g.init(dir.path(), None).await.unwrap();
    let data = tempfile::tempdir().unwrap();
    let v = vcs(&g, data.path());
    let err = v
        .add_worktree(
            ProjectId::new(),
            dir.path(),
            AddWorktreeRequest { agent_id: Some(AgentId::new()), ..Default::default() },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, VcsError::InvalidArgument(_)), "{err}");
    // Empty repo: log is empty, status works.
    assert!(g.log(dir.path(), None, None, 0, 10).await.unwrap().commits.is_empty());
    assert!(g.status(dir.path()).await.unwrap().head.is_none());
}
