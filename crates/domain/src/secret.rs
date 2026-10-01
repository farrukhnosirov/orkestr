//! Secret references (§24). The domain only ever holds *references*; secret
//! values live in the OS keyring or encrypted vault (`orkestr-security`).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::common::Timestamp;
use crate::ids::{EnvironmentId, ProjectId, SecretRefId};

/// Where a secret value is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum SecretBackend {
    /// Windows Credential Manager / macOS Keychain / Linux Secret Service.
    OsKeyring,
    /// Local encrypted vault file (chacha20poly1305, key in keyring).
    EncryptedVault,
    /// Self-hosted vault (future).
    SelfHostedVault,
    /// Cloud secret manager (future).
    CloudSecretManager,
}

/// Visibility scope of a secret. Agents never see secrets of another project
/// without permission (§67).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(tag = "scope", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum SecretScope {
    /// Global (e.g. provider API keys).
    Global,
    /// One project.
    Project {
        /// Project.
        project_id: ProjectId,
    },
    /// One environment of a project.
    Environment {
        /// Project.
        project_id: ProjectId,
        /// Environment.
        environment_id: EnvironmentId,
    },
}

/// Reference to a secret. Safe to store in SQLite and to send over the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SecretReference {
    /// Id.
    pub id: SecretRefId,
    /// Human name ("OPENAI_API_KEY").
    pub name: String,
    /// Scope.
    pub scope: SecretScope,
    /// Backend.
    pub backend: SecretBackend,
    /// Locator inside the backend (keyring service/account or vault key). Not secret.
    pub locator: String,
    /// Creation time.
    pub created_at: Timestamp,
    /// Last rotation.
    pub rotated_at: Option<Timestamp>,
    /// Last access (audited).
    pub last_accessed_at: Option<Timestamp>,
}
