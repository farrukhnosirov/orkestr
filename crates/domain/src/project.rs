//! Projects, repositories, workspaces and snapshots (§13, §25, §56).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::capability::ApprovalMode;
use crate::common::{Actor, Budget, Timestamp};
use crate::ids::{AgentId, ProjectId, ProviderId, RepositoryId, SnapshotId, TaskId, WorkspaceId};

/// Data classification of a project (§56). Ordered: `Public < … < Secret`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum DataClassification {
    /// Public / open source.
    Public,
    /// Internal company code (default).
    #[default]
    Internal,
    /// Confidential customer/business data.
    Confidential,
    /// Secret: strictest handling.
    Secret,
}

/// Which AI providers may see project data (§56).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "mode", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum AiPolicy {
    /// Any configured cloud or local provider.
    #[default]
    CloudAllowed,
    /// Only the listed provider instances.
    SelectedProvidersOnly {
        /// Allowed providers.
        providers: Vec<ProviderId>,
    },
    /// Only local models (e.g. Ollama) — nothing leaves the machine.
    LocalModelsOnly,
}

/// Where project state may be synchronized (§56, §45).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum SyncPolicy {
    /// Managed cloud or self-hosted sync allowed.
    #[default]
    Allowed,
    /// Only a self-hosted control plane may receive data.
    SelfHostedOnly,
    /// No remote sync at all.
    NoRemoteSync,
}

/// Per-project privacy policy (§56).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PrivacyPolicy {
    /// AI provider policy.
    pub ai: AiPolicy,
    /// Sync policy.
    pub sync: SyncPolicy,
}

impl PrivacyPolicy {
    /// Whether a provider may receive this project's data.
    ///
    /// `is_local` must be true only for providers running on a node the user controls.
    #[must_use]
    pub fn allows_provider(&self, provider: ProviderId, is_local: bool) -> bool {
        match &self.ai {
            AiPolicy::CloudAllowed => true,
            AiPolicy::SelectedProvidersOnly { providers } => providers.contains(&provider),
            AiPolicy::LocalModelsOnly => is_local,
        }
    }

    /// Whether data may be synced to a remote backend.
    #[must_use]
    pub fn allows_remote_sync(&self, self_hosted: bool) -> bool {
        match self.sync {
            SyncPolicy::Allowed => true,
            SyncPolicy::SelfHostedOnly => self_hosted,
            SyncPolicy::NoRemoteSync => false,
        }
    }
}

/// A project (§13).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Project {
    /// Id.
    pub id: ProjectId,
    /// Display name.
    pub name: String,
    /// URL/filename-safe slug derived from the name.
    pub slug: String,
    /// Free-form description.
    pub description: String,
    /// Root directory on the local node, if bound to a folder.
    pub root_path: Option<String>,
    /// Primary repository.
    pub repository_id: Option<RepositoryId>,
    /// Privacy policy (§56).
    pub privacy: PrivacyPolicy,
    /// Data classification (§56).
    pub classification: DataClassification,
    /// Default approval mode for agents in this project.
    pub approval_mode: ApprovalMode,
    /// Project budget (§22).
    pub budget: Option<Budget>,
    /// Accent color token or hex (`#RRGGBB`) for the project rail.
    pub color: Option<String>,
    /// Icon name (lucide) for the project rail.
    pub icon: Option<String>,
    /// Free tags.
    pub tags: Vec<String>,
    /// Archived projects are hidden from the rail.
    pub archived: bool,
    /// Creation time.
    pub created_at: Timestamp,
    /// Last modification time.
    pub updated_at: Timestamp,
    /// Last time the project was opened in any client.
    pub last_opened_at: Option<Timestamp>,
}

impl Project {
    /// Creates a project with sensible defaults (Internal, Ask, cloud allowed).
    #[must_use]
    pub fn new(name: impl Into<String>, now: Timestamp) -> Self {
        let name = name.into();
        Self {
            id: ProjectId::new(),
            slug: slugify(&name),
            name,
            description: String::new(),
            root_path: None,
            repository_id: None,
            privacy: PrivacyPolicy::default(),
            classification: DataClassification::default(),
            approval_mode: ApprovalMode::default(),
            budget: None,
            color: None,
            icon: None,
            tags: Vec::new(),
            archived: false,
            created_at: now,
            updated_at: now,
            last_opened_at: None,
        }
    }
}

/// Produces a lowercase ASCII slug (`"Payroll Module!"` → `"payroll-module"`).
#[must_use]
pub fn slugify(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut last_dash = true;
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        out.push_str("project");
    }
    out
}

/// Version control system of a repository.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum VcsKind {
    /// Git (the only supported VCS, §16).
    #[default]
    Git,
}

/// A source repository attached to a project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Repository {
    /// Id.
    pub id: RepositoryId,
    /// Owning project.
    pub project_id: ProjectId,
    /// VCS kind.
    pub vcs: VcsKind,
    /// Local path of the main checkout.
    pub path: String,
    /// Remote URL (credentials are never stored here).
    pub remote_url: Option<String>,
    /// Default branch, e.g. `main`.
    pub default_branch: String,
    /// Creation time.
    pub created_at: Timestamp,
}

/// Kind of workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum WorkspaceKind {
    /// The user's main checkout.
    Main,
    /// A git worktree, usually owned by an agent (§16).
    Worktree {
        /// Branch checked out in the worktree.
        branch: String,
        /// Owning agent, if any.
        agent_id: Option<AgentId>,
    },
}

/// A working directory of a repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Workspace {
    /// Id.
    pub id: WorkspaceId,
    /// Owning project.
    pub project_id: ProjectId,
    /// Repository the workspace belongs to.
    pub repository_id: RepositoryId,
    /// Absolute path.
    pub path: String,
    /// Main checkout or worktree.
    pub kind: WorkspaceKind,
    /// Creation time.
    pub created_at: Timestamp,
}

/// Why a snapshot was taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum SnapshotReason {
    /// Explicit user request.
    Manual,
    /// Automatic checkpoint before a risky/autonomous change (§25).
    BeforeRiskyAction,
    /// Task checkpoint (also used for node migration §39).
    TaskCheckpoint,
    /// Taken right before a rollback so the rollback itself is undoable.
    PreRollback,
    /// Periodic sync snapshot (§41).
    Periodic,
}

/// A workspace snapshot including uncommitted state (§25). Not a git commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WorkspaceSnapshot {
    /// Id.
    pub id: SnapshotId,
    /// Owning project.
    pub project_id: ProjectId,
    /// Snapshotted workspace.
    pub workspace_id: WorkspaceId,
    /// Task this snapshot belongs to (for task rollback).
    pub task_id: Option<TaskId>,
    /// Human label.
    pub label: String,
    /// Reason.
    pub reason: SnapshotReason,
    /// HEAD commit at snapshot time.
    pub base_commit: Option<String>,
    /// Object id of the manifest (file list + blob ids) in the object store.
    pub manifest_object_id: String,
    /// Whether uncommitted/untracked changes are included.
    pub includes_uncommitted: bool,
    /// Number of files captured.
    pub file_count: u64,
    /// Total bytes captured (before dedup).
    pub size_bytes: u64,
    /// Who created it.
    pub created_by: Actor,
    /// Creation time.
    pub created_at: Timestamp,
    /// Absolute path of the captured directory (additive, Phase 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub workspace_path: Option<String>,
    /// Branch checked out at snapshot time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub branch: Option<String>,
    /// Whether the git worktree had uncommitted changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub dirty: Option<bool>,
    /// Agent the snapshot belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub agent_id: Option<AgentId>,
    /// Before/after marker of a task checkpoint pair (task rollback, §63.J).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub task_phase: Option<TaskSnapshotPhase>,
    /// Bytes newly written to the object store (after dedup).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub new_bytes: Option<u64>,
}

/// Position of a snapshot in a task's checkpoint pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum TaskSnapshotPhase {
    /// Taken before the task changed anything.
    Before,
    /// Taken when the task finished or stopped.
    After,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_cases() {
        assert_eq!(slugify("Payroll Module!"), "payroll-module");
        assert_eq!(slugify("  --Hello__World--  "), "hello-world");
        assert_eq!(slugify("Школа"), "project");
        assert_eq!(slugify("a1 b2"), "a1-b2");
    }

    #[test]
    fn privacy_policy_rules() {
        let p1 = ProviderId::new();
        let p2 = ProviderId::new();
        let mut policy = PrivacyPolicy::default();
        assert!(policy.allows_provider(p1, false));
        policy.ai = AiPolicy::SelectedProvidersOnly { providers: vec![p1] };
        assert!(policy.allows_provider(p1, false));
        assert!(!policy.allows_provider(p2, true));
        policy.ai = AiPolicy::LocalModelsOnly;
        assert!(!policy.allows_provider(p1, false));
        assert!(policy.allows_provider(p1, true));
        policy.sync = SyncPolicy::SelfHostedOnly;
        assert!(policy.allows_remote_sync(true));
        assert!(!policy.allows_remote_sync(false));
        policy.sync = SyncPolicy::NoRemoteSync;
        assert!(!policy.allows_remote_sync(true));
    }

    #[test]
    fn project_json_is_camel_case() {
        let p = Project::new("Demo", crate::common::now());
        let v = serde_json::to_value(&p).unwrap();
        assert!(v.get("approvalMode").is_some());
        assert_eq!(v["privacy"]["ai"]["mode"], "CloudAllowed");
        assert_eq!(v["classification"], "Internal");
        let back: Project = serde_json::from_value(v).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn classification_is_ordered() {
        assert!(DataClassification::Public < DataClassification::Secret);
    }
}
