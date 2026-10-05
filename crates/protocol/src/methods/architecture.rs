//! `architecture.*` — the Architecture Map (§21, §68.48).
//!
//! The core scans the project root and builds a model of the project:
//! modules / packages (Cargo, npm, Python, Go, .NET, plain directories),
//! services (binaries, Dockerfiles, docker-compose services), databases
//! (compose images, SQLite usage, migration folders, ORM schemas), APIs
//! (HTTP route declarations and OpenAPI documents) and dependencies (internal
//! edges from manifests and imports; external dependencies grouped per
//! ecosystem). Every scan that changes the model is persisted as a snapshot;
//! snapshots can be diffed.
//!
//! The map refreshes incrementally (only changed files are re-parsed) after
//! `FileChanged` events, debounced; a changed map publishes
//! `ArchitectureMapUpdated` on `project:<id>`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{ProjectId, Timestamp};

use super::method;

/// Max snapshots kept per project (older ones are pruned).
pub const MAX_ARCH_SNAPSHOTS: usize = 50;

/// What a map node represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ArchNodeKind {
    /// A deployable / runnable unit: binary, Dockerfile image, compose service.
    Service,
    /// A module / package / crate / project / top-level source directory.
    Module,
    /// An HTTP API surface (routes of a module, or an OpenAPI document).
    Api,
    /// A database or data store (compose image, SQLite, migrations, ORM schema).
    Database,
    /// A group of external (third-party) dependencies of one ecosystem.
    External,
}

/// What a map edge represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ArchEdgeKind {
    /// Manifest dependency between internal modules (path / workspace deps).
    DependsOn,
    /// Source import of another internal module (not declared in the manifest).
    Imports,
    /// Uses a database / data store.
    Uses,
    /// Exposes an API.
    Exposes,
    /// A service is built from / runs a module; a compose service depends on another.
    Runs,
    /// Uses third-party dependencies of an ecosystem group.
    External,
}

/// One HTTP route found in source code or an OpenAPI document.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ApiRoute {
    /// HTTP method in upper case (`GET`, `POST`, …; `ANY` when unknown).
    pub method: String,
    /// Route path as declared (`/users/{id}`).
    pub path: String,
    /// File (relative to the project root, `/` separators).
    pub file: String,
    /// 1-based line.
    pub line: u32,
    /// Framework / source (`axum`, `actix`, `express`, `fastapi`, `flask`,
    /// `aspnet`, `gin`, `net/http`, `openapi`, …).
    pub framework: String,
}

/// A node of the map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ArchNode {
    /// Stable id (`module:cargo:orkestr-core`, `db:compose:postgres`, …);
    /// identical across scans so snapshots can be diffed.
    pub id: String,
    /// Kind.
    pub kind: ArchNodeKind,
    /// Display name (package / service / database name).
    pub name: String,
    /// Ecosystem / origin (`cargo`, `npm`, `python`, `go`, `dotnet`, `dir`,
    /// `docker`, `compose`, `sqlite`, `openapi`, …).
    pub ecosystem: String,
    /// Directory (relative to the project root, `/` separators; `.` = root).
    #[serde(default)]
    pub path: Option<String>,
    /// Short technical detail (version, image, framework, …). Not translated.
    #[serde(default)]
    pub detail: Option<String>,
    /// Key files (manifests, entry points, route files), relative paths, capped.
    #[serde(default)]
    pub files: Vec<String>,
    /// Number of source files that belong to this node (modules).
    #[serde(default)]
    pub file_count: u32,
    /// Free-form tags (`binary`, `library`, `workspace-root`, `test`, …).
    #[serde(default)]
    pub tags: Vec<String>,
    /// Routes (API nodes).
    #[serde(default)]
    pub routes: Vec<ApiRoute>,
    /// Dependency names (External group nodes), sorted.
    #[serde(default)]
    pub dependencies: Vec<String>,
}

/// An edge of the map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ArchEdge {
    /// Stable id (`<from>-><to>:<kind>`).
    pub id: String,
    /// Source node id.
    pub from: String,
    /// Target node id.
    pub to: String,
    /// Kind.
    pub kind: ArchEdgeKind,
    /// Strength (number of imports / dependencies behind the edge, ≥ 1).
    pub weight: u32,
    /// Evidence (dependency names, sample import paths), capped.
    #[serde(default)]
    pub labels: Vec<String>,
}

/// Counts of a map.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ArchStats {
    /// Module nodes.
    pub modules: u32,
    /// Service nodes.
    pub services: u32,
    /// Database nodes.
    pub databases: u32,
    /// API nodes.
    pub apis: u32,
    /// Routes over all API nodes.
    pub routes: u32,
    /// External dependency names over all groups.
    pub external_dependencies: u32,
    /// Edges.
    pub edges: u32,
    /// Files walked by the scan.
    pub files_scanned: u32,
    /// Files (re-)parsed by the scan (incremental scans parse only changes).
    pub files_parsed: u32,
    /// Scan duration (ms).
    pub scan_ms: u64,
}

/// The architecture model of a project at one point in time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ArchitectureMap {
    /// Snapshot id.
    pub snapshot_id: String,
    /// Project.
    pub project_id: ProjectId,
    /// Scan time.
    pub generated_at: Timestamp,
    /// Content digest of nodes + edges (equal digests = same model).
    pub digest: String,
    /// Nodes, sorted by kind then id.
    pub nodes: Vec<ArchNode>,
    /// Edges, sorted by id.
    pub edges: Vec<ArchEdge>,
    /// Counts.
    pub stats: ArchStats,
    /// Problems met while scanning (unparsable manifests, …). Not translated.
    #[serde(default)]
    pub warnings: Vec<String>,
    /// The scan hit its file limit; the model is partial.
    #[serde(default)]
    pub truncated: bool,
}

/// A persisted snapshot (history list).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ArchSnapshotInfo {
    /// Snapshot id.
    pub snapshot_id: String,
    /// Scan time.
    pub generated_at: Timestamp,
    /// Digest.
    pub digest: String,
    /// Counts.
    pub stats: ArchStats,
}

/// A node whose attributes changed between two snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ArchNodeChange {
    /// Node id.
    pub id: String,
    /// Kind.
    pub kind: ArchNodeKind,
    /// Name (new snapshot).
    pub name: String,
    /// Changed attributes (`detail`, `files`, `fileCount`, `tags`, `routes`,
    /// `dependencies`, `path`, `name`).
    pub fields: Vec<String>,
    /// Routes added (API nodes).
    #[serde(default)]
    pub added_routes: Vec<ApiRoute>,
    /// Routes removed (API nodes).
    #[serde(default)]
    pub removed_routes: Vec<ApiRoute>,
    /// Dependencies added (External groups).
    #[serde(default)]
    pub added_dependencies: Vec<String>,
    /// Dependencies removed (External groups).
    #[serde(default)]
    pub removed_dependencies: Vec<String>,
}

/// Difference between two snapshots.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ArchDiff {
    /// Older snapshot (`None` = empty model).
    pub from_snapshot_id: Option<String>,
    /// Newer snapshot.
    pub to_snapshot_id: Option<String>,
    /// Nodes only in the newer snapshot.
    pub added_nodes: Vec<ArchNode>,
    /// Nodes only in the older snapshot.
    pub removed_nodes: Vec<ArchNode>,
    /// Nodes in both with different attributes.
    pub changed_nodes: Vec<ArchNodeChange>,
    /// Edges only in the newer snapshot.
    pub added_edges: Vec<ArchEdge>,
    /// Edges only in the older snapshot.
    pub removed_edges: Vec<ArchEdge>,
}

impl ArchDiff {
    /// Nothing changed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.added_nodes.is_empty()
            && self.removed_nodes.is_empty()
            && self.changed_nodes.is_empty()
            && self.added_edges.is_empty()
            && self.removed_edges.is_empty()
    }
}

/// Params of `architecture.map`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ArchMapParams {
    /// Project.
    pub project_id: ProjectId,
    /// A stored snapshot; default the latest (a first scan runs when the
    /// project has none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub snapshot_id: Option<String>,
}

/// Result of `architecture.map`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ArchMapResult {
    /// The map.
    pub map: ArchitectureMap,
    /// Stored snapshots, newest first.
    pub snapshots: Vec<ArchSnapshotInfo>,
}

/// Params of `architecture.refresh`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ArchRefreshParams {
    /// Project.
    pub project_id: ProjectId,
    /// Re-parse every file instead of only changed ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub full: Option<bool>,
}

/// Result of `architecture.refresh`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ArchRefreshResult {
    /// Latest map (the new snapshot, or the previous one when unchanged).
    pub map: ArchitectureMap,
    /// A new snapshot was stored.
    pub changed: bool,
    /// Difference to the previous snapshot (when one existed).
    pub diff: Option<ArchDiff>,
}

/// Params of `architecture.diff`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ArchDiffParams {
    /// Project.
    pub project_id: ProjectId,
    /// Older snapshot; default the one before `to`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub from: Option<String>,
    /// Newer snapshot; default the latest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub to: Option<String>,
}

/// Result of `architecture.diff`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ArchDiffResult {
    /// The diff.
    pub diff: ArchDiff,
}

method!(
    /// `architecture.map`.
    Map, "architecture.map", ArchMapParams, ArchMapResult
);
method!(
    /// `architecture.refresh`.
    Refresh, "architecture.refresh", ArchRefreshParams, ArchRefreshResult
);
method!(
    /// `architecture.diff`.
    Diff, "architecture.diff", ArchDiffParams, ArchDiffResult
);
