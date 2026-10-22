//! Phase 6 WP-D: content-addressed blob transfer + local model distribution
//! core → core over `LoopbackLink` (ADR-0039): 20 MiB mock model distributed
//! and verified, tampered chunk rejected (nothing visible), interrupted
//! transfer resumed, installed model listed for the scheduler, `node.blobGet`
//! pull, coordinator-only enforcement for remote callers.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use orkestr_core::nodes::LoopbackLink;
use orkestr_core::rpc::models::{self as wpd, StaticLinks};
use orkestr_core::rpc::{build_router, Principal, RpcContext, RpcRouter};
use orkestr_core::{CoreConfig, CoreHandle};
use orkestr_domain::{CorrelationId, NodeId, SessionId};
use orkestr_node::blobs::{pull_blob, BlobStore, MemBlobStore};
use orkestr_node::{NodeError, NodeLink};
use orkestr_protocol::event::EventKind;
use orkestr_protocol::Event;
use serde_json::{json, Value};

const MIB: u64 = 1024 * 1024;

async fn call(router: &RpcRouter, core: &CoreHandle, method: &str, params: Value) -> Result<Value, String> {
    let ctx = RpcContext {
        services: Arc::clone(&core.services),
        session_id: SessionId::new(),
        correlation_id: CorrelationId::new(),
        principal: Principal::Local,
    };
    router.dispatch(ctx, method, params).await.map_err(|e| format!("{e:?}"))
}

/// Wraps a link: fails `node.blobPut` after `fail_after` puts, or flips a byte
/// in put number `corrupt`.
struct FaultyLink {
    inner: Arc<dyn NodeLink>,
    puts: AtomicUsize,
    fail_after: Option<usize>,
    corrupt: Option<usize>,
}

#[async_trait]
impl NodeLink for FaultyLink {
    fn node_id(&self) -> NodeId {
        self.inner.node_id()
    }
    async fn call(&self, method: &str, mut params: Value) -> Result<Value, NodeError> {
        if method == "node.blobPut" {
            let n = self.puts.fetch_add(1, Ordering::SeqCst);
            if self.fail_after.is_some_and(|f| n >= f) {
                return Err(NodeError::Transport("link dropped".into()));
            }
            if self.corrupt == Some(n) {
                let b64 = base64::engine::general_purpose::STANDARD;
                let mut data = b64.decode(params["data"].as_str().unwrap()).unwrap();
                data[100] ^= 0x5a;
                params["data"] = json!(b64.encode(data));
            }
        }
        self.inner.call(method, params).await
    }
    fn events(&self) -> tokio::sync::broadcast::Receiver<Event> {
        self.inner.events()
    }
}

fn wire(coord: &CoreHandle, worker_id: NodeId, link: Arc<dyn NodeLink>) {
    let links = StaticLinks(HashMap::from([(worker_id, link)]));
    wpd::set_link_provider(&coord.services, Arc::new(links)).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn distribute_mock_model_core_to_core() {
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let coord = orkestr_core::start(CoreConfig::with_data_dir(da.path())).await.unwrap();
    let worker = orkestr_core::start(CoreConfig::with_data_dir(db.path())).await.unwrap();
    let router = Arc::new(build_router());
    let worker_id = worker.services.node_id;
    let loopback: Arc<dyn NodeLink> =
        Arc::new(LoopbackLink::new(worker_id, Arc::clone(&router), Arc::clone(&worker.services), Principal::Local));
    let mut events = coord.services.events.subscribe_all();

    // Import a deterministic 20 MiB mock model on the coordinator.
    let imported =
        call(&router, &coord, "models.import", json!({"name": "mock-coder", "tag": "20m", "mockSizeBytes": 20 * MIB}))
            .await
            .unwrap();
    let digest = imported["model"]["digest"].as_str().unwrap().to_owned();
    let layer = imported["model"]["manifest"]["layers"][0]["digest"].as_str().unwrap().to_owned();
    assert_eq!(imported["model"]["sizeBytes"], 20 * MIB);
    let listed = call(&router, &coord, "models.list", json!({})).await.unwrap();
    assert_eq!(listed["models"].as_array().unwrap().len(), 1);
    let params = json!({"modelRef": "mock-coder:20m", "nodeIds": [worker_id]});

    // Default (node hub) links: the worker is not enrolled → per-node failure, not a crash.
    let r = call(&router, &coord, "models.distribute", params.clone()).await.unwrap();
    assert_eq!(r["outcomes"][0]["ok"], false);
    assert!(r["outcomes"][0]["error"].as_str().unwrap().contains("not found"), "{r}");

    // Tampered chunk: finalize fails on the worker, nothing visible.
    wire(
        &coord,
        worker_id,
        Arc::new(FaultyLink {
            inner: Arc::clone(&loopback),
            puts: AtomicUsize::new(0),
            fail_after: None,
            corrupt: Some(3),
        }),
    );
    let r = call(&router, &coord, "models.distribute", params.clone()).await.unwrap();
    assert_eq!(r["outcomes"][0]["ok"], false, "{r}");
    assert!(r["outcomes"][0]["error"].as_str().unwrap().contains("digest mismatch"), "{r}");
    let has = call(&router, &worker, "node.blobHas", json!({"digest": layer})).await.unwrap();
    assert_eq!(has, json!({"present": false, "received": 0}));
    assert!(call(&router, &worker, "models.list", json!({})).await.unwrap()["models"].as_array().unwrap().is_empty());
    let worker_blobs = wpd::blob_store(&worker.services).unwrap();
    let ld = orkestr_domain::Digest(layer.clone());
    assert!(worker_blobs.path(&ld).is_none());

    // Interrupted transfer: link drops after 10 chunks (5 MiB).
    wire(
        &coord,
        worker_id,
        Arc::new(FaultyLink {
            inner: Arc::clone(&loopback),
            puts: AtomicUsize::new(0),
            fail_after: Some(10),
            corrupt: None,
        }),
    );
    let r = call(&router, &coord, "models.distribute", params.clone()).await.unwrap();
    assert_eq!(r["outcomes"][0]["ok"], false);
    let has = call(&router, &worker, "node.blobHas", json!({"digest": layer})).await.unwrap();
    assert_eq!(has["received"], 5 * MIB);

    // Resume over a healthy link: only the remaining 15 MiB move.
    wire(&coord, worker_id, Arc::clone(&loopback));
    let r = call(&router, &coord, "models.distribute", params.clone()).await.unwrap();
    assert_eq!(r["digest"], json!(digest));
    assert_eq!(r["outcomes"][0]["ok"], true, "{r}");
    assert_eq!(r["outcomes"][0]["bytesSent"], 15 * MIB);

    // Installed + verified on the worker; visible to the scheduler feed.
    let wl = call(&router, &worker, "models.list", json!({})).await.unwrap();
    assert_eq!(wl["models"][0]["modelRef"], "mock-coder:20m");
    assert_eq!(wl["models"][0]["digest"], json!(digest));
    assert_eq!(wl["models"][0]["ollamaInstalled"], false, "mock models never go to Ollama");
    let local = wpd::local_models(&worker.services);
    assert_eq!(local.len(), 1);
    assert_eq!(local[0].name, "mock-coder:20m");
    assert_eq!(local[0].size_bytes, Some(20 * MIB));
    assert_eq!(wpd::installed_digests(&worker.services), vec![orkestr_domain::Digest(digest.clone())]);
    let worker_bytes = std::fs::read(worker_blobs.path(&ld).unwrap()).unwrap();
    let coord_bytes = std::fs::read(wpd::blob_store(&coord.services).unwrap().path(&ld).unwrap()).unwrap();
    assert!(worker_bytes == coord_bytes, "byte-identical copy");

    // Re-distribute: nothing to send, still ok (idempotent).
    let r = call(&router, &coord, "models.distribute", params.clone()).await.unwrap();
    assert_eq!(r["outcomes"][0]["bytesSent"], 0);

    // ModelDistributed events: 3 failures + 2 successes.
    let mut seen = Vec::new();
    while let Ok(Some(rcv)) = tokio::time::timeout(std::time::Duration::from_millis(200), events.recv()).await {
        if let orkestr_events::Received::Event(ev) = rcv {
            if let EventKind::ModelDistributed(p) = &ev.kind {
                assert_eq!(p.node_id, worker_id);
                seen.push(p.ok);
            }
        }
    }
    assert_eq!(seen, vec![false, false, false, true, true]);

    // Pull counterpart (node.blobGet) into a fresh store, verified.
    let mem = MemBlobStore::new();
    assert_eq!(pull_blob(loopback.as_ref(), &mem, &ld).await.unwrap(), 20 * MIB);
    assert!(mem.has(&ld).await);

    // Install with a forged manifest digest is refused.
    let mut manifest = wl["models"][0]["manifest"].clone();
    manifest["tag"] = json!("forged");
    let e = call(&router, &worker, "node.modelInstall", json!({"manifest": manifest, "digest": digest})).await;
    assert!(e.unwrap_err().contains("digest mismatch"));

    // Bad inputs.
    assert!(call(&router, &coord, "models.import", json!({"name": "x", "tag": "y"})).await.is_err());
    assert!(call(&router, &coord, "models.import", json!({"name": "x", "tag": "y", "path": "../../etc"}))
        .await
        .is_err());
    assert!(call(&router, &coord, "models.distribute", json!({"modelRef": "nope:1", "nodeIds": [worker_id]}))
        .await
        .unwrap_err()
        .contains("not found"));
    let big = base64::engine::general_purpose::STANDARD.encode(vec![0u8; 600 * 1024]);
    let e =
        call(&router, &worker, "node.blobPut", json!({"digest": layer, "offset": 0, "total": 1, "data": big})).await;
    assert!(e.unwrap_err().contains("too large"));

    coord.shutdown().await;
    worker.shutdown().await;
}
