//! Local model store + distribution (`models.*`) and the worker-side blob
//! transfer / install methods (`node.blobHas|blobPut|blobGet|modelInstall`)
//! (Phase 6, ADR-0039). **Owner: WP-D.**
//!
//! * Blobs live in the shared [`FsBlobStore`] of this core
//!   (`ORKESTR_HOME/blobs/sha256/<hex>`), obtained with [`blob_store`] — WP-C
//!   uses the same instance for checkpoint/artifact blobs.
//! * Installed models are indexed by [`orkestr_node::models::ModelStore`]
//!   (`ORKESTR_HOME/models`). For scheduling, the heartbeat / hello (WP-A)
//!   reports [`local_models`] (→ `NodeInfo.localModels.models`) and
//!   [`installed_digests`] (→ `NodeSnapshot.modelDigests`).
//! * `models.distribute` needs coordinator → worker links. **Integrator:**
//!   call [`set_link_provider`] once at startup with a [`LinkProvider`] backed
//!   by the WP-A connector/registry (tests use [`StaticLinks`] with
//!   `LoopbackLink`s). Without a provider every target fails with
//!   "node links not wired".
//! * `node.*` methods are coordinator-only for remote callers via
//!   `MethodPolicy::worker()` (protocol policy table).
//!
//! Progress: per-layer transfer progress is logged (`tracing`); one
//! `ModelDistributed` event is published per target node.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use base64::Engine as _;
use orkestr_domain::NodeId;
use orkestr_node::blobs::{push_blob_counted, BlobError, BlobStore, FsBlobStore};
use orkestr_node::link::call_typed;
use orkestr_node::models::{self as store, ModelError, ModelStore};
use orkestr_node::{NodeError, NodeLink};
use orkestr_protocol::event::EventKind;
use orkestr_protocol::methods::models as m;
use orkestr_protocol::methods::nodes::LocalModel;
use orkestr_protocol::{NewEvent, RpcError};
use parking_lot::{Mutex, RwLock};

use super::{RpcContext, RpcRouter};
use crate::services::Services;

/// Opens coordinator → worker links for `models.distribute`.
#[async_trait]
pub trait LinkProvider: Send + Sync {
    /// Link to `node_id` (enrolled worker).
    async fn link(&self, node_id: NodeId) -> Result<Arc<dyn NodeLink>, NodeError>;
}

/// A fixed set of links (tests, simple setups).
#[derive(Default)]
pub struct StaticLinks(pub HashMap<NodeId, Arc<dyn NodeLink>>);

#[async_trait]
impl LinkProvider for StaticLinks {
    async fn link(&self, node_id: NodeId) -> Result<Arc<dyn NodeLink>, NodeError> {
        self.0.get(&node_id).cloned().ok_or(NodeError::NotFound(node_id))
    }
}

struct ModelHost {
    blobs: Arc<FsBlobStore>,
    links: RwLock<Option<Arc<dyn LinkProvider>>>,
}

/// Per-core state keyed by data dir (several cores may share one process in tests).
fn hosts() -> &'static Mutex<HashMap<PathBuf, Arc<ModelHost>>> {
    static HOSTS: OnceLock<Mutex<HashMap<PathBuf, Arc<ModelHost>>>> = OnceLock::new();
    HOSTS.get_or_init(Mutex::default)
}

fn host(services: &Services) -> Result<Arc<ModelHost>, RpcError> {
    let home = home(services);
    let mut map = hosts().lock();
    if let Some(h) = map.get(home) {
        return Ok(Arc::clone(h));
    }
    let blobs = Arc::new(FsBlobStore::open(home).map_err(blob_err)?);
    let h = Arc::new(ModelHost { blobs, links: RwLock::new(None) });
    map.insert(home.to_path_buf(), Arc::clone(&h));
    Ok(h)
}

fn home(services: &Services) -> &Path {
    &services.paths.data_dir
}

/// The shared blob store of this core (`ORKESTR_HOME/blobs`).
///
/// # Errors
/// Directory creation failure.
pub fn blob_store(services: &Services) -> Result<Arc<FsBlobStore>, RpcError> {
    Ok(Arc::clone(&host(services)?.blobs))
}

/// Installs the link provider used by `models.distribute` (integrator: WP-A).
///
/// # Errors
/// Directory creation failure of the blob store.
pub fn set_link_provider(services: &Services, provider: Arc<dyn LinkProvider>) -> Result<(), RpcError> {
    *host(services)?.links.write() = Some(provider);
    Ok(())
}

/// Installed models of this core as `NodeInfo.localModels.models` rows.
#[must_use]
pub fn local_models(services: &Services) -> Vec<LocalModel> {
    store::local_models(home(services))
}

/// Manifest digests of installed models (`NodeSnapshot.modelDigests`).
#[must_use]
pub fn installed_digests(services: &Services) -> Vec<orkestr_domain::Digest> {
    store::installed_digests(home(services))
}

fn blob_err(e: BlobError) -> RpcError {
    match e {
        BlobError::InvalidDigest(_) | BlobError::ChunkTooLarge(_) | BlobError::TooLarge(_) => {
            RpcError::bad_request(e.to_string())
        }
        BlobError::BadOffset { .. } | BlobError::DigestMismatch(_) => RpcError::conflict(e.to_string()),
        BlobError::NotFound(_) => RpcError::not_found(e.to_string()),
        BlobError::Unimplemented(f) => RpcError::unimplemented_feature(f),
        BlobError::Io(_) | BlobError::Transfer(_) => RpcError::internal(e.to_string()),
    }
}

fn model_err(e: ModelError) -> RpcError {
    match e {
        ModelError::Invalid(_) => RpcError::bad_request(e.to_string()),
        ModelError::DigestMismatch { .. } | ModelError::MissingLayer(_) | ModelError::LayerSize { .. } => {
            RpcError::conflict(e.to_string())
        }
        ModelError::NotFound(_) => RpcError::not_found(e.to_string()),
        ModelError::Blob(b) => blob_err(b),
        ModelError::Io(_) => RpcError::internal(e.to_string()),
    }
}

fn model_store(services: &Services) -> Result<ModelStore, RpcError> {
    ModelStore::open(home(services)).map_err(model_err)
}

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::List, _, _>(|ctx: RpcContext, _p| async move {
            Ok(m::ListModelsResult { models: model_store(&ctx.services)?.list() })
        })
        .method::<m::Import, _, _>(import)
        .method::<m::Distribute, _, _>(distribute)
        .method::<m::BlobHas, _, _>(|ctx: RpcContext, p: m::BlobHasParams| async move {
            if !p.digest.is_valid() {
                return Err(RpcError::bad_request(format!("invalid digest {}", p.digest)));
            }
            let blobs = blob_store(&ctx.services)?;
            Ok(m::BlobHasResult { present: blobs.has(&p.digest).await, received: blobs.received(&p.digest).await })
        })
        .method::<m::BlobPut, _, _>(|ctx: RpcContext, p: m::BlobPutParams| async move {
            // Reject oversize payloads before decoding.
            if p.data.len() > (m::BLOB_CHUNK_SIZE as usize).div_ceil(3) * 4 {
                return Err(blob_err(BlobError::ChunkTooLarge(p.data.len() / 4 * 3)));
            }
            let data = base64::engine::general_purpose::STANDARD
                .decode(p.data.as_bytes())
                .map_err(|e| RpcError::bad_request(format!("invalid base64: {e}")))?;
            let blobs = blob_store(&ctx.services)?;
            let received = blobs.put_chunk(&p.digest, p.offset, p.total, &data).await.map_err(blob_err)?;
            let complete = received == p.total && blobs.has(&p.digest).await;
            Ok(m::BlobPutResult { received, complete })
        })
        .method::<m::BlobGet, _, _>(|ctx: RpcContext, p: m::BlobGetParams| async move {
            let blobs = blob_store(&ctx.services)?;
            let total = blobs.size(&p.digest).await.ok_or_else(|| blob_err(BlobError::NotFound(p.digest.clone())))?;
            let data = blobs.get_chunk(&p.digest, p.offset, p.len).await.map_err(blob_err)?;
            Ok(m::BlobGetResult { total, data: base64::engine::general_purpose::STANDARD.encode(data) })
        })
        .method::<m::ModelInstall, _, _>(|ctx: RpcContext, p: m::ModelInstallParams| async move {
            let blobs = blob_store(&ctx.services)?;
            let ollama = orkestr_node::ollama::find_binary();
            let model = store::install(&model_store(&ctx.services)?, blobs.as_ref(), &p.manifest, &p.digest, ollama.as_deref())
                .await
                .map_err(model_err)?;
            tracing::info!(model = %model.model_ref, digest = %model.digest, ollama = model.ollama_installed, "model installed");
            Ok(m::ModelResult { model })
        });
}

async fn import(ctx: RpcContext, p: m::ImportModelParams) -> Result<m::ModelResult, RpcError> {
    let blobs = blob_store(&ctx.services)?;
    let ms = model_store(&ctx.services)?;
    let model = match (p.path.as_deref(), p.mock_size_bytes) {
        (None, Some(size)) => store::import_mock(&ms, blobs.as_ref(), blobs.staging_dir(), &p.name, &p.tag, size).await,
        (Some(path), None) => {
            let ollama = orkestr_node::ollama::find_binary();
            let root = home(&ctx.services).join("import");
            store::import_gguf_file(&ms, blobs.as_ref(), &root, Path::new(path), &p.name, &p.tag, ollama.as_deref())
                .await
        }
        _ => return Err(RpcError::bad_request("exactly one of `path` or `mockSizeBytes` is required")),
    }
    .map_err(model_err)?;
    Ok(m::ModelResult { model })
}

/// Maximum targets of one `models.distribute`.
const MAX_TARGETS: usize = 64;

async fn distribute(ctx: RpcContext, p: m::DistributeModelParams) -> Result<m::DistributeModelResult, RpcError> {
    let model = model_store(&ctx.services)?
        .get(&p.model_ref)
        .ok_or_else(|| RpcError::not_found(format!("model {} not found", p.model_ref)))?;
    let mut targets: Vec<NodeId> = Vec::new();
    for n in p.node_ids {
        if !targets.contains(&n) {
            targets.push(n);
        }
    }
    if targets.is_empty() || targets.len() > MAX_TARGETS {
        return Err(RpcError::bad_request(format!("nodeIds must list 1..={MAX_TARGETS} nodes")));
    }
    let host = host(&ctx.services)?;
    let provider = host.links.read().clone();
    let mut outcomes = Vec::with_capacity(targets.len());
    for node_id in targets {
        let res = if node_id == ctx.services.node_id {
            Ok(0) // Already in this core's store.
        } else {
            distribute_one(host.blobs.as_ref(), provider.as_deref(), node_id, &model).await
        };
        let outcome = match res {
            Ok(bytes_sent) => m::DistributeOutcome { node_id, ok: true, bytes_sent, error: None },
            Err(e) => {
                tracing::warn!(model = %model.model_ref, node = %node_id, error = %e, "model distribution failed");
                m::DistributeOutcome { node_id, ok: false, bytes_sent: 0, error: Some(e) }
            }
        };
        let payload = m::ModelDistributedPayload {
            model_ref: model.model_ref.clone(),
            digest: model.digest.clone(),
            node_id,
            ok: outcome.ok,
        };
        if let Err(e) = ctx.services.events.publish(NewEvent::new(EventKind::ModelDistributed(payload))).await {
            tracing::warn!(error = %e, "ModelDistributed not published");
        }
        outcomes.push(outcome);
    }
    Ok(m::DistributeModelResult { digest: model.digest, outcomes })
}

async fn distribute_one(
    blobs: &dyn BlobStore,
    provider: Option<&dyn LinkProvider>,
    node_id: NodeId,
    model: &m::StoredModel,
) -> Result<u64, String> {
    let provider = provider.ok_or_else(|| "node links not wired".to_owned())?;
    let link = provider.link(node_id).await.map_err(|e| e.to_string())?;
    let mut sent = 0u64;
    let layers = model.manifest.layers.len();
    for (i, layer) in model.manifest.layers.iter().enumerate() {
        sent += push_blob_counted(blobs, link.as_ref(), &layer.digest).await.map_err(|e| e.to_string())?;
        tracing::info!(model = %model.model_ref, node = %node_id, layer = i + 1, layers, bytes = layer.size, "model layer transferred");
    }
    let installed = call_typed::<m::ModelInstall>(
        link.as_ref(),
        &m::ModelInstallParams { manifest: model.manifest.clone(), digest: model.digest.clone() },
    )
    .await
    .map_err(|e| e.to_string())?;
    if installed.model.digest != model.digest {
        return Err(format!("worker installed {} instead of {}", installed.model.digest, model.digest));
    }
    Ok(sent)
}
