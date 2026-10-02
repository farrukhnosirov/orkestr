//! # orkestr-vcs
//!
//! Git via the `git` CLI (MASTER_SPEC §16, ADR-0008/0009,
//! `docs/architecture/git-worktree-strategy.md`):
//!
//! * [`runner`] — spawning git: argument vectors only (never a shell),
//!   no prompts, timeouts, `*.lock` retry with backoff;
//! * [`parse`] — `-z` / porcelain v2 parsers (status, numstat, name-status,
//!   log, for-each-ref, worktree list, merge-tree);
//! * [`cli::GitCli`] — status, diff, log, show, branches, stage/unstage/discard,
//!   commit, worktrees, merge preview (`merge-tree --write-tree`), `--no-ff`
//!   merge, abort, init — with a per-repository async mutex for mutations;
//! * [`service::Vcs`] — project-level service: worktree resolution and
//!   Orkestr-managed agent worktrees (`agent/<role>-<shortid>` under
//!   `<data_dir>/worktrees/<project short>/<agent short>`);
//! * [`store`] — the `vcs_worktrees` table and its migration.
//!
//! Git is separate from live sync (§40) and Orkestr never auto-commits (§64).
//! Destructive operations (discard, force delete/remove) require an explicit
//! confirmation from the caller.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod cli;
pub mod error;
pub mod naming;
pub mod parse;
pub mod runner;
pub mod service;
pub mod store;

pub use cli::{DiffRequest, GitCli, RepoInfo, RepoLocks};
pub use error::{VcsError, VcsResult};
pub use parse::GitVersion;
pub use runner::{GitOutput, GitRunner, RunOptions};
pub use service::{AddWorktreeRequest, Vcs};

/// Migrations owned by this crate.
#[must_use]
pub fn vcs_migrations() -> Vec<orkestr_storage::Migration> {
    store::MIGRATIONS.to_vec()
}
