# ADR-0021: Snapshots as content manifests in the object store (not git commits or stashes)

- **Status:** Accepted
- **Date:** 2026-10-02
- **Phase:** 1 (create, task rollback, workspace rollback), 2 (automatic checkpoints in self-healing loops), 4 (sync of snapshots)
- **Spec:** §25 (snapshot includes uncommitted state, ≠ Git commit; Rollback Task / Rollback Workspace; rollback does not destroy unrelated changes), §40–42, §62.14, §63J, §68.34–35, §70 #2
- **Review:** RR-C5, RR-A4
- **Spike:** SP-16 · **Risks:** R-14, R-21

## Context

Before risky or autonomous changes, Orkestr must checkpoint the workspace, including uncommitted and untracked files. "Rollback Task" must undo exactly one task's changes and keep everyone else's (§63J). Snapshots must not pollute Git: no commits, refs or stash entries (§40, §64 "auto-commit spam").

## Decision drivers

1. Capture uncommitted, staged and untracked (non-ignored) state.
2. Zero footprint in the user's Git: no refs, no stash, no index changes.
3. Cheap and deduplicated: snapshots are frequent.
4. Task-precise rollback with conflict detection, never a blind overwrite (§70).
5. Sync-ready content addressing (Phase 4).

## Considered options

| Option | Verdict |
|---|---|
| **Manifest of changed files vs HEAD + blobs in the Orkestr object store (blake3), plus Git HEAD/branch/index metadata** | **Chosen** |
| `git stash create` + private ref (`refs/orkestr/snapshots/*`) | Rejected. Writes into the user's repo object DB and refs, is visible to tools and `git gc` interaction, and fails "≠ Git commit" in spirit |
| Full copy of the worktree | Rejected. Too slow and too large |
| Filesystem snapshots (VSS / APFS / btrfs) | Not portable, needs privileges |

## Decision

### Model

```rust
pub struct Snapshot {
    pub id: SnapshotId, pub project_id: ProjectId, pub worktree_path: PathBuf,
    pub task_id: Option<TaskId>, pub reason: SnapshotReason,     // Manual | BeforeTask | BeforeDangerousAction | BeforeRollback | BeforeMerge | Checkpoint | EmergencyStop
    pub head: Option<CommitId>, pub branch: Option<String>,
    pub manifest: ObjectId,                                       // JSON manifest blob in object store
    pub stats: SnapshotStats,                                     // files, bytes, new_bytes (dedupe)
    pub created_at: DateTime<Utc>, pub created_by: ActorRef,
}
// manifest JSON
{ "version":1, "head":"<oid>", "branch":"agent/backend-3f9a1c2e",
  "index": [{"path":"src/a.rs","mode":"100644","oid":"<git blob oid>","stage":0}],   // only entries that differ from HEAD (staged state)
  "entries": [ {"path":"src/a.rs","state":"modified","mode":"100644","object":"b3:…","size":1234},
               {"path":"new.txt","state":"untracked","mode":"100644","object":"b3:…","size":10},
               {"path":"old.txt","state":"deleted"} ],
  "excluded": {"ignoredByGitignore": true, "overSizeLimit": ["assets/big.bin"]} }
```

- **Capture:**
  1. Run `git status --porcelain=v2 -z --untracked-files=all`, which gives modified, deleted, untracked (not ignored) and staged files.
  2. Read each changed file and `put_file` it into the object store.
  3. Record the HEAD oid and the branch.
  The cost is proportional to the *changed* set, not the repo.
  - Files over `snapshots.maxFileBytes` (default 50 MiB) are listed in `overSizeLimit` and not captured, with a warning in the UI.
  - Ignored files (`node_modules`, `target`) are excluded by design.
  - Non-git projects fall back to a full-tree manifest that respects a `.orkestrignore`.
- **Consistency:**
  - Capture runs while holding the worktree's **agent write lock**. Agent tools on that worktree wait (milliseconds to seconds), and user edits are not blocked.
  - Files are re-hashed after copying. If the mtime or size changed during the copy, the file is retried, up to 3 times. After that, the snapshot is marked `partial` with the path listed.

### Task rollback (§63J): no destruction of unrelated changes

Inputs: the snapshot `S0` taken `BeforeTask(T)`, the set `F(T)` of files the task touched (from `FileChanged` events with `taskId = T` plus the task's commits' diff), and each file's post-task hash `H_T(f)` recorded when the task completed or stopped.

```
for f in F(T):
    cur = hash(current(f))
    if cur == H_T(f):            restore f to S0(f) (or delete if absent in S0)   // only T changed it
    elif cur == S0(f):           nothing to do                                      // already reverted
    else:                        CONFLICT(f)  → do not touch; offer 3-way merge (S0 base, T version, current)
for commits made by T on its agent branch:
    if branch is T's own agent branch and unmerged: reset branch to S0.head (branch-local, no user branch touched)
    if merged into a user branch: create a revert commit proposal (approval required), never history rewrite
```

- Before any rollback, an automatic `BeforeRollback` snapshot is taken, so the rollback itself can be undone (RR-A4).
- The result emits `RollbackPerformed{snapshotId, taskId?, restored:[…], conflicts:[…], skipped:[…]}`.
- Workspace rollback (all files in the manifest plus `git reset --soft <head>` on the **same** branch if HEAD moved) is classified Dangerous, so it always asks.
- **Retention:** keep everything for 7 days, then daily snapshots for 30 days, then those pinned by tasks or the user. Objects are collected by GC (ADR-0007).

## Consequences

- **Positive:** the user's Git stays untouched; capture cost scales with changes; rollback is precise and safe with explicit conflicts; snapshots are content-addressed and sync-ready.
- **Negative:** ignored files are not restored by default. A deleted `.env` that is in `.gitignore` is the typical case. Mitigation: the project setting `snapshots.includeIgnored` (glob list, default `[".env", ".env.*"]`) captures selected ignored files. Files that match `security.secretFilePatterns` (default `.env*`, `*.pem`, `*.key`) are stored **encrypted with the vault data key** (`SecretStore::encrypt_blob`/`decrypt_blob`, an extension of ADR-0011). They are never stored in plaintext in the object store and are excluded from Phase 4 sync unless synced to a self-hosted vault.
- **Negative:** `F(T)` must be complete. File-change attribution comes from the runtime (writes through tools are attributed exactly) and from a worktree watcher (`notify` crate) for writes by child processes (build tools, CLI agents). In a dedicated worktree every change is the task's. In a shared `UserCheckout`, watcher attribution is approximate, and conflicts are surfaced rather than guessed.

## Implementation notes (Phase 0 foundation, 2026-10-02)

Not implemented yet. `orkestr-snapshots` has the skeleton `SnapshotEngine { create(workspace, task, reason, label, by), rollback_task(task), rollback_workspace(snapshot) }` and `RollbackReport { files_restored, pre_rollback_snapshot }`. The domain `SnapshotReason` is `Manual | BeforeRiskyAction | TaskCheckpoint | PreRollback | Periodic`: the names used above map as `BeforeTask` → `TaskCheckpoint`, `BeforeDangerousAction` → `BeforeRiskyAction`, `BeforeRollback` → `PreRollback`; reasons such as `EmergencyStop`/`BeforeMerge` would be additive variants. `WorkspaceSnapshot.manifestObjectId` is a plain string holding the bare-hex `ObjectId`. The object store (zstd, verified reads) exists. SP-16 has not run.
