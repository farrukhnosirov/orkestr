//! `secrets.*` — secret references and the vault (§24). Values can be set
//! but are **never** returned over RPC.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{ProjectId, SecretRefId, SecretReference, SecretScope};

use super::method;
use super::permissions::DeletedResult;

/// Params of `secrets.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListSecretsParams {
    /// Only Global + this project's secrets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
}

/// Result of `secrets.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListSecretsResult {
    /// References (no values).
    pub secrets: Vec<SecretReference>,
}

/// Params of `secrets.set` (create or rotate; name unique per scope).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SetSecretParams {
    /// Name (`[A-Za-z_][A-Za-z0-9_.-]{0,127}`).
    pub name: String,
    /// Scope.
    pub scope: SecretScope,
    /// Value (write-only; never logged or echoed).
    pub value: String,
}

/// Result of `secrets.set`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SecretResult {
    /// The reference.
    pub secret: SecretReference,
}

/// Params of `secrets.delete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeleteSecretParams {
    /// Secret.
    pub secret_id: SecretRefId,
}

/// Value backend preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum SecretBackendPreference {
    /// Keyring first, vault fallback.
    Auto,
    /// Encrypted vault.
    Vault,
    /// Encrypted vault with key file.
    VaultFileKey,
    /// In-memory (tests).
    Memory,
}

/// Vault status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VaultStatusResult {
    /// Locked.
    pub locked: bool,
    /// Backend preference.
    pub backend: SecretBackendPreference,
    /// Number of secrets.
    pub secret_count: u32,
}

method!(
    /// `secrets.list`.
    List, "secrets.list", ListSecretsParams, ListSecretsResult
);
method!(
    /// `secrets.set` — audited; the value is registered for redaction.
    Set, "secrets.set", SetSecretParams, SecretResult
);
method!(
    /// `secrets.delete` — audited.
    Delete, "secrets.delete", DeleteSecretParams, DeletedResult
);
method!(
    /// `secrets.lock` — Lock Vault (§52).
    Lock, "secrets.lock", super::Empty, VaultStatusResult
);
method!(
    /// `secrets.unlock`.
    Unlock, "secrets.unlock", super::Empty, VaultStatusResult
);
method!(
    /// `secrets.status`.
    Status, "secrets.status", super::Empty, VaultStatusResult
);
