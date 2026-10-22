//! Registry index document (`index.json`) and its detached signature (`index.sig`).

use serde::{Deserialize, Serialize};

use orkestr_protocol::methods::marketplace::PackageKind;

/// Index format version.
pub const INDEX_FORMAT: u32 = 1;

/// One package version in an index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexEntry {
    /// Package id.
    pub id: String,
    /// Kind.
    pub kind: PackageKind,
    /// Display name.
    pub name: String,
    /// Version.
    pub version: String,
    /// Publisher.
    pub publisher: String,
    /// Description.
    #[serde(default)]
    pub description: String,
    /// Signing key id.
    pub key_id: String,
    /// Package sha256 (hex).
    pub sha256: String,
    /// Package URL / path relative to the index.
    pub url: String,
    /// Revoked.
    #[serde(default)]
    pub revoked: bool,
}

/// `index.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexDocument {
    /// Format version ([`INDEX_FORMAT`]).
    pub format: u32,
    /// Registry name.
    pub name: String,
    /// Entries.
    pub entries: Vec<IndexEntry>,
    /// Revoked key ids.
    #[serde(default)]
    pub revoked_keys: Vec<String>,
}
