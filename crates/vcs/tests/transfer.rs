//! Bundle / patch helpers for remote job workspaces (Phase 6).

use std::path::Path;

use orkestr_vcs::{GitCli, RunOptions};

async fn sh(g: &GitCli, dir: &Path, args: &[&str]) -> String {
    g.runner().run_ok(dir, args, RunOptions::write()).await.unwrap().text()
}

#[tokio::test]
async fn bundle_clone_patch_roundtrip() {
    let g = GitCli::discover();
    if !g.is_available() {
        eprintln!("git not installed — skipping");
        return;
    }
    let src = tempfile::tempdir().unwrap();
    let s = src.path();
    g.init(s, Some("main")).await.unwrap();
    for (k, v) in [("user.name", "T"), ("user.email", "t@example.invalid"), ("core.autocrlf", "false")] {
        sh(&g, s, &["config", k, v]).await;
    }
    std::fs::write(s.join("a.txt"), "one\n").unwrap();
    std::fs::write(s.join("bin.dat"), [0u8, 1, 2, 3]).unwrap();
    sh(&g, s, &["add", "-A"]).await;
    sh(&g, s, &["commit", "-qm", "init"]).await;
    let head = sh(&g, s, &["rev-parse", "HEAD"]).await.trim().to_owned();
    // dirty: modified tracked + untracked text + untracked binary
    std::fs::write(s.join("a.txt"), "one\ntwo\n").unwrap();
    std::fs::write(s.join("new.txt"), "fresh\n").unwrap();
    std::fs::create_dir_all(s.join("sub")).unwrap();
    std::fs::write(s.join("sub/b.bin"), [9u8, 0, 9]).unwrap();

    let out = tempfile::tempdir().unwrap();
    let bundle = out.path().join("ws.bundle");
    g.create_bundle(s, &bundle).await.unwrap();
    let patch = g.working_patch(s, &head).await.unwrap();
    assert!(!patch.is_empty());
    // the source index is left as it was (untracked stays untracked)
    let st = sh(&g, s, &["status", "--porcelain"]).await;
    assert!(st.contains("?? new.txt"), "{st}");

    let target = out.path().join("clone");
    g.clone_bundle(&bundle, &target, &head, Some("main")).await.unwrap();
    assert_eq!(std::fs::read_to_string(target.join("a.txt")).unwrap(), "one\n");
    g.apply_patch(&target, &patch).await.unwrap();
    assert_eq!(std::fs::read_to_string(target.join("a.txt")).unwrap(), "one\ntwo\n");
    assert_eq!(std::fs::read_to_string(target.join("new.txt")).unwrap(), "fresh\n");
    assert_eq!(std::fs::read(target.join("sub/b.bin")).unwrap(), vec![9u8, 0, 9]);
    // clean worktree → empty patch; empty patch applies as a no-op
    let clean = g.working_patch(&target, &head).await.unwrap();
    assert!(!clean.is_empty(), "clone carries the applied changes");
    g.apply_patch(s, b"").await.unwrap();
}
