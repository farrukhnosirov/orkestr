//! `models.*` — local model store and distribution (Phase 6, ADR-0039), and the
//! worker-side blob transfer primitive `node.blob*` (content-addressed
//! SHA-256 blobs in 512 KiB chunks over the E2E session).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{Digest, NodeId, Timestamp};

use super::method;

/// Maximum chunk size of `node.blobPut` / `node.blobGet` (stays under the 1 MiB E2E record limit).
pub const BLOB_CHUNK_SIZE: u32 = 512 * 1024;

/// One layer of a model manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ModelLayer {
    /// Blob digest.
    pub digest: Digest,
    /// Size in bytes.
    pub size: u64,
    /// Media type (`application/vnd.orkestr.model.gguf`, `…params+json`).
    pub media_type: String,
}

/// A model manifest (content-addressed: its own digest identifies it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ModelManifest {
    /// Name (`qwen2.5-coder`).
    pub name: String,
    /// Tag (`7b-q4`).
    pub tag: String,
    /// Format (`gguf`, `mock`).
    pub format: String,
    /// Layers.
    pub layers: Vec<ModelLayer>,
}

/// A model in the local store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StoredModel {
    /// `name:tag`.
    pub model_ref: String,
    /// Manifest digest.
    pub digest: Digest,
    /// Manifest.
    pub manifest: ModelManifest,
    /// Total bytes.
    pub size_bytes: u64,
    /// Registered with Ollama.
    pub ollama_installed: bool,
    /// Installed at.
    pub installed_at: Timestamp,
}

/// `models.list` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListModelsResult {
    /// Models in the local store.
    pub models: Vec<StoredModel>,
}

/// `models.import` params (a local GGUF file, or a generated mock of `mockSizeBytes`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImportModelParams {
    /// Name.
    pub name: String,
    /// Tag.
    pub tag: String,
    /// Path of a GGUF file on this machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub path: Option<String>,
    /// Generate a deterministic mock model of this size instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub mock_size_bytes: Option<u64>,
}

/// Result carrying one model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ModelResult {
    /// The model.
    pub model: StoredModel,
}

/// `models.distribute` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DistributeModelParams {
    /// `name:tag`.
    pub model_ref: String,
    /// Target nodes.
    pub node_ids: Vec<NodeId>,
}

/// Outcome per node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DistributeOutcome {
    /// Node.
    pub node_id: NodeId,
    /// Installed and verified.
    pub ok: bool,
    /// Bytes transferred.
    pub bytes_sent: u64,
    /// Error detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub error: Option<String>,
}

/// `models.distribute` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DistributeModelResult {
    /// Manifest digest.
    pub digest: Digest,
    /// Per node.
    pub outcomes: Vec<DistributeOutcome>,
}

/// `node.blobHas` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BlobHasParams {
    /// Blob.
    pub digest: Digest,
}

/// `node.blobHas` result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BlobHasResult {
    /// Verified and complete.
    pub present: bool,
    /// Bytes already received of a partial upload (resume offset).
    pub received: u64,
}

/// `node.blobPut` params: one chunk; the blob is verified and finalized when `received == total`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BlobPutParams {
    /// Blob.
    pub digest: Digest,
    /// Offset of this chunk.
    pub offset: u64,
    /// Total size.
    pub total: u64,
    /// Chunk bytes, base64 (≤ [`BLOB_CHUNK_SIZE`] decoded).
    pub data: String,
}

/// `node.blobPut` result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BlobPutResult {
    /// Bytes received so far.
    pub received: u64,
    /// Finalized (hash verified).
    pub complete: bool,
}

/// `node.blobGet` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BlobGetParams {
    /// Blob.
    pub digest: Digest,
    /// Offset.
    pub offset: u64,
    /// Max bytes (≤ [`BLOB_CHUNK_SIZE`]).
    pub len: u32,
}

/// `node.blobGet` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BlobGetResult {
    /// Total size.
    pub total: u64,
    /// Chunk bytes, base64.
    pub data: String,
}

/// `node.modelInstall` params (all layers already pushed with `node.blobPut`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ModelInstallParams {
    /// Manifest.
    pub manifest: ModelManifest,
    /// Expected manifest digest.
    pub digest: Digest,
}

/// `ModelDistributed` event payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ModelDistributedPayload {
    /// `name:tag`.
    pub model_ref: String,
    /// Manifest digest.
    pub digest: Digest,
    /// Target node.
    pub node_id: NodeId,
    /// Verified + installed.
    pub ok: bool,
}

method!(
    /// `models.list` — local model store.
    List, "models.list", super::Empty, ListModelsResult
);
method!(
    /// `models.import` — import a GGUF (or mock) model into the local store.
    Import, "models.import", ImportModelParams, ModelResult
);
method!(
    /// `models.distribute` — push a stored model to nodes (verified by digest).
    Distribute, "models.distribute", DistributeModelParams, DistributeModelResult
);
method!(
    /// Worker side `node.blobHas`.
    BlobHas, "node.blobHas", BlobHasParams, BlobHasResult
);
method!(
    /// Worker side `node.blobPut`.
    BlobPut, "node.blobPut", BlobPutParams, BlobPutResult
);
method!(
    /// Worker side `node.blobGet`.
    BlobGet, "node.blobGet", BlobGetParams, BlobGetResult
);
method!(
    /// Worker side `node.modelInstall`.
    ModelInstall, "node.modelInstall", ModelInstallParams, ModelResult
);
