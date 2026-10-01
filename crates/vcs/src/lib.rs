//! # orkestr-vcs — **SKELETON** (Phase 1 fills it in)
//!
//! Git via the `git` CLI (MASTER_SPEC §16): status, diff, branches, worktrees
//! (one per parallel agent: `agent/<role>`), merge, conflict detection. Git is
//! separate from live sync (§40) and Orkestr never auto-commits (§64).
//! Destructive git commands are classified by `orkestr-security`'s guard.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Errors.
#[derive(Debug, thiserror::Error)]
pub enum VcsError {
    /// `git` not installed.
    #[error("git executable not found")]
    GitNotFound,
    /// Not a repository.
    #[error("not a git repository: {0}")]
    NotARepository(String),
    /// Command failure.
    #[error("git {command} failed: {stderr}")]
    Command {
        /// Subcommand.
        command: String,
        /// Stderr.
        stderr: String,
    },
    /// Merge conflict.
    #[error("merge conflict in {0:?}")]
    Conflict(Vec<String>),
    /// Explicit stub marker.
    #[error("unimplemented-feature: {0}")]
    Unimplemented(&'static str),
}

/// File status entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileStatus {
    /// Path relative to the repo root.
    pub path: String,
    /// Index status letter (porcelain v2).
    pub index: char,
    /// Worktree status letter.
    pub worktree: char,
}

/// Repository status.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoStatus {
    /// Current branch (None when detached).
    pub branch: Option<String>,
    /// HEAD commit.
    pub head: Option<String>,
    /// Commits ahead of upstream.
    pub ahead: u32,
    /// Commits behind upstream.
    pub behind: u32,
    /// Changed files.
    pub files: Vec<FileStatus>,
}

/// A git worktree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Worktree {
    /// Path.
    pub path: PathBuf,
    /// Branch.
    pub branch: Option<String>,
    /// HEAD.
    pub head: String,
}

/// Git operations.
#[async_trait]
pub trait Git: Send + Sync {
    /// `git status --porcelain=v2 --branch`.
    async fn status(&self, repo: &Path) -> Result<RepoStatus, VcsError>;
    /// Unified diff of the working tree (or against `base`).
    async fn diff(&self, repo: &Path, base: Option<&str>) -> Result<String, VcsError>;
    /// Branch names.
    async fn branches(&self, repo: &Path) -> Result<Vec<String>, VcsError>;
    /// Lists worktrees.
    async fn worktrees(&self, repo: &Path) -> Result<Vec<Worktree>, VcsError>;
    /// Creates `branch` from `base` in a new worktree at `path`.
    async fn add_worktree(&self, repo: &Path, path: &Path, branch: &str, base: &str) -> Result<Worktree, VcsError>;
    /// Removes a worktree (refuses when it has uncommitted changes unless `force`).
    async fn remove_worktree(&self, repo: &Path, path: &Path, force: bool) -> Result<(), VcsError>;
}
