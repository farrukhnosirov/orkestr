//! `marketplace.*` — signed package registries (bundled official catalogue,
//! file directory, self-hosted HTTP at `orkestr-gateway /registry/*`),
//! publisher key trust (TOFU + pinning + revocation), search / install /
//! publish (Phase 7, ADR-0040). **Owner: WP2.**

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{PermissionRequest, Timestamp};

use super::plugins::PluginInfo;
use super::{method, Empty};

/// Kind of a registry source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum RegistrySourceKind {
    /// Bundled official catalogue (embedded key).
    Official,
    /// Local directory with `index.json` + `index.sig`.
    Directory,
    /// HTTP(S) registry (`<url>/index.json`), e.g. an `orkestr-gateway`.
    Http,
}

/// A configured registry source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RegistrySource {
    /// Id (`official` for the bundled catalogue).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Kind.
    pub kind: RegistrySourceKind,
    /// Directory path or base URL.
    pub location: String,
    /// Enabled.
    pub enabled: bool,
    /// Entries in the last verified index.
    pub entries: u32,
    /// Last successful refresh.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub last_refresh: Option<Timestamp>,
    /// Last refresh error (signature, network, parse).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub last_error: Option<String>,
}

/// Kind of a registry package.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum PackageKind {
    /// Code / declarative plugin (`.orkp`).
    Plugin,
    /// Widget-only package (`.orkp` with kind `Widget`).
    Widget,
    /// Agent template (JSON, imported via `templates.import`).
    AgentTemplate,
}

/// Trust state of a publisher key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum KeyTrust {
    /// Seen, not trusted: installs need `marketplace.trustKey`.
    Untrusted,
    /// Trusted by the user (trust on first use) and pinned for updates.
    Trusted,
    /// Built-in official key.
    Official,
    /// Revoked by an index revocation list or the user.
    Revoked,
}

/// A publisher signing key (ed25519).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PublisherKey {
    /// Key id (first 16 hex chars of the sha256 of the public key).
    pub key_id: String,
    /// Publisher name.
    pub publisher: String,
    /// Public key (base64).
    pub public_key: String,
    /// Human-comparable fingerprint (`ab12 cd34 ...`).
    pub fingerprint: String,
    /// Trust.
    pub trust: KeyTrust,
    /// First seen.
    pub first_seen: Timestamp,
}

/// One entry of a verified registry index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RegistryEntry {
    /// Package id (`publisher.name`).
    pub id: String,
    /// Kind.
    pub kind: PackageKind,
    /// Display name.
    pub name: String,
    /// Version (semver).
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
    /// Package URL / relative path.
    pub url: String,
    /// Revoked by the index.
    #[serde(default)]
    pub revoked: bool,
    /// Source the entry came from.
    pub source_id: String,
    /// Installed version, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub installed_version: Option<String>,
}

/// `marketplace.sources` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SourcesResult {
    /// Sources.
    pub sources: Vec<RegistrySource>,
}

/// `marketplace.addSource` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AddSourceParams {
    /// Kind (`Directory` or `Http`).
    pub kind: RegistrySourceKind,
    /// Directory path or base URL.
    pub location: String,
    /// Display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub name: Option<String>,
}

/// Result carrying one source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SourceResult {
    /// The source.
    pub source: RegistrySource,
}

/// Params naming one source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SourceIdParams {
    /// Source.
    pub source_id: String,
}

/// `marketplace.refresh` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RefreshSourcesParams {
    /// One source (default: all enabled).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub source_id: Option<String>,
}

/// `marketplace.search` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MarketplaceSearchParams {
    /// Free text (id, name, description, publisher).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub query: Option<String>,
    /// Kind filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub kind: Option<PackageKind>,
    /// Source filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub source_id: Option<String>,
    /// Include revoked entries.
    #[serde(default)]
    pub include_revoked: bool,
    /// Max results (default 100).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<u32>,
}

/// `marketplace.search` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MarketplaceSearchResult {
    /// Latest version of each matching package.
    pub entries: Vec<RegistryEntry>,
}

/// `marketplace.get` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GetEntryParams {
    /// Package id.
    pub id: String,
    /// Source (default: first source that has it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub source_id: Option<String>,
}

/// `marketplace.get` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EntryDetailsResult {
    /// Latest entry.
    pub entry: RegistryEntry,
    /// All versions in the index (newest first).
    pub versions: Vec<String>,
    /// Permissions the package will request (from the index metadata).
    pub permissions: Vec<PermissionRequest>,
    /// Signing key.
    pub key: PublisherKey,
}

/// `marketplace.install` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct InstallEntryParams {
    /// Package id.
    pub id: String,
    /// Version (default latest).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub version: Option<String>,
    /// Source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub source_id: Option<String>,
}

/// `marketplace.install` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct InstallEntryResult {
    /// Kind installed.
    pub kind: PackageKind,
    /// Installed version.
    pub version: String,
    /// Plugin / widget package (kinds `Plugin`, `Widget`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub plugin: Option<PluginInfo>,
    /// Permissions to decide with `plugins.grant`.
    #[serde(default)]
    pub pending_grants: Vec<PermissionRequest>,
    /// Imported agent template ids (kind `AgentTemplate`).
    #[serde(default)]
    pub template_ids: Vec<String>,
}

/// `marketplace.publish` params: sign a local package with the vault-held
/// publisher key and add it to a writable source (directory or gateway).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PublishPackageParams {
    /// Package file (`.orkp`) or agent template JSON.
    pub path: String,
    /// Kind.
    pub kind: PackageKind,
    /// Target source.
    pub source_id: String,
    /// Publisher name (first publish creates the key).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub publisher: Option<String>,
}

/// `marketplace.publish` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PublishPackageResult {
    /// The new entry.
    pub entry: RegistryEntry,
}

/// `marketplace.keys` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct KeysResult {
    /// Known publisher keys.
    pub keys: Vec<PublisherKey>,
    /// This installation's publisher key (vault-held), if created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub local_publisher: Option<PublisherKey>,
}

/// `marketplace.trustKey` params (the user confirms the fingerprint).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TrustKeyParams {
    /// Key id.
    pub key_id: String,
    /// Fingerprint as shown to the user (must match).
    pub fingerprint: String,
}

/// `marketplace.untrustKey` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct KeyIdParams {
    /// Key id.
    pub key_id: String,
}

/// Result carrying one key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct KeyResult {
    /// The key.
    pub key: PublisherKey,
}

/// `MarketplaceUpdated`: a source was refreshed / added / removed, or a package was installed or published.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MarketplaceUpdatedPayload {
    /// Source affected (`None` = several).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub source_id: Option<String>,
    /// What happened (`refreshed`, `sourceAdded`, `sourceRemoved`, `installed`, `published`, `keyTrusted`, `keyUntrusted`).
    pub change: String,
}

method!(/// Configured registry sources.
    Sources, "marketplace.sources", Empty, SourcesResult);
method!(/// Adds a directory / HTTP source (local only).
    AddSource, "marketplace.addSource", AddSourceParams, SourceResult);
method!(/// Removes a source.
    RemoveSource, "marketplace.removeSource", SourceIdParams, Empty);
method!(/// Fetches and verifies indexes.
    Refresh, "marketplace.refresh", RefreshSourcesParams, SourcesResult);
method!(/// Searches verified indexes.
    Search, "marketplace.search", MarketplaceSearchParams, MarketplaceSearchResult);
method!(/// Package details.
    Get, "marketplace.get", GetEntryParams, EntryDetailsResult);
method!(/// Installs a package (local only).
    Install, "marketplace.install", InstallEntryParams, InstallEntryResult);
method!(/// Publishes a local package (local only).
    Publish, "marketplace.publish", PublishPackageParams, PublishPackageResult);
method!(/// Publisher keys.
    Keys, "marketplace.keys", Empty, KeysResult);
method!(/// Trusts (pins) a publisher key (local only).
    TrustKey, "marketplace.trustKey", TrustKeyParams, KeyResult);
method!(/// Removes trust from a publisher key.
    UntrustKey, "marketplace.untrustKey", KeyIdParams, KeyResult);
