//! # orkestr-security — **SKELETON** (Phase 1 fills it in)
//!
//! Responsibility (MASTER_SPEC §23, §24, §52; contract §6):
//!
//! * [`PermissionEngine`] — **every** agent tool call goes through
//!   `check(actor, capability, resource, context) -> Decision`. Scopes
//!   Global ⊃ Project ⊃ Agent ⊃ Task; narrowest explicit rule wins; `Deny`
//!   beats `Allow` at the same scope; approval modes `Ask / SemiAuto /
//!   FullAuto` (FullAuto never overrides hard policies).
//! * [`DangerousActionGuard`] — classifies commands/actions (recursive delete,
//!   `DROP`/`TRUNCATE`, prod deploys, secret changes, `git push --force`,
//!   `git reset --hard`, `git clean -fd`, format/diskpart, registry edits, …)
//!   for PowerShell, cmd and POSIX shells. Dangerous ⇒ always `Ask`;
//!   hard-denied ⇒ `Deny`. Must be unit-tested with many cases.
//! * [`PathGuard`] — confines filesystem tools to a root (rejects `..`,
//!   symlink escapes, absolute paths outside root, UNC, `\\?\`).
//! * [`SecretStore`] — secrets via the OS keyring (`orkestr-platform`
//!   `CredentialStore`) with an encrypted-file fallback (chacha20poly1305, key
//!   in keyring). The DB only stores `SecretReference`s.
//! * [`Redactor`] — scrubs secrets from logs, events and tool I/O.
//! * Audit — every decision becomes an `AuditEvent`.
//!
//! Nothing here is implemented yet; the types below fix the public surface so
//! other crates can be written against it.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

use async_trait::async_trait;
use orkestr_domain::{Actor, AgentId, ApprovalMode, Capability, ProjectId, SecretReference, TaskId};
use serde::{Deserialize, Serialize};

/// Result of a permission check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Decision {
    /// Proceed.
    Allow,
    /// Ask the user (reason shown in the approval).
    Ask(String),
    /// Refuse (reason shown to the agent/user).
    Deny(String),
}

/// Context of an action being checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionContext {
    /// Project of the action.
    pub project_id: Option<ProjectId>,
    /// Acting agent.
    pub agent_id: Option<AgentId>,
    /// Task being worked on.
    pub task_id: Option<TaskId>,
    /// Effective approval mode.
    pub approval_mode: ApprovalMode,
    /// Whether the target environment is protected (production).
    pub protected_environment: bool,
}

/// The permission engine (contract §6).
pub trait PermissionEngine: Send + Sync {
    /// Decides whether `actor` may use `capability` on `resource`.
    fn check(&self, actor: &Actor, capability: Capability, resource: &str, ctx: &ActionContext) -> Decision;
}

/// Classification of a command or action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DangerClass {
    /// Nothing dangerous detected.
    Safe,
    /// Dangerous: always requires approval, even in FullAuto.
    Dangerous(String),
    /// Hard-denied.
    Forbidden(String),
}

/// Shell dialect of a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ShellDialect {
    /// PowerShell 5/7.
    PowerShell,
    /// cmd.exe.
    Cmd,
    /// bash/zsh/sh/fish.
    Posix,
}

/// Dangerous action guard.
pub trait DangerousActionGuard: Send + Sync {
    /// Classifies a shell command line.
    fn classify_command(&self, command: &str, dialect: ShellDialect) -> DangerClass;
    /// Classifies a SQL statement.
    fn classify_sql(&self, sql: &str) -> DangerClass;
}

/// Filesystem confinement.
pub trait PathGuard: Send + Sync {
    /// Resolves `requested` inside `root`, rejecting any escape.
    fn confine(&self, root: &std::path::Path, requested: &str) -> Result<std::path::PathBuf, SecurityError>;
}

/// Secret storage on top of the OS keyring / encrypted vault.
#[async_trait]
pub trait SecretStore: Send + Sync {
    /// Stores a secret value and returns its reference.
    async fn put(&self, name: &str, value: &str) -> Result<SecretReference, SecurityError>;
    /// Reads a secret (audited).
    async fn get(&self, reference: &SecretReference) -> Result<String, SecurityError>;
    /// Deletes a secret.
    async fn delete(&self, reference: &SecretReference) -> Result<(), SecurityError>;
}

/// Secret redaction.
pub trait Redactor: Send + Sync {
    /// Returns `text` with secrets replaced by `[REDACTED]`.
    fn redact(&self, text: &str) -> String;
}

/// Security errors.
#[derive(Debug, thiserror::Error)]
pub enum SecurityError {
    /// Path escapes its root.
    #[error("path escapes the allowed root: {0}")]
    PathEscape(String),
    /// Secret not found.
    #[error("secret not found: {0}")]
    SecretNotFound(String),
    /// Vault locked (emergency control §52).
    #[error("vault is locked")]
    VaultLocked,
    /// Backend failure.
    #[error("backend: {0}")]
    Backend(String),
    /// Explicit stub marker.
    #[error("unimplemented-feature: {0}")]
    Unimplemented(&'static str),
}
