# ADR-0039 — Local model distribution

Status: accepted — implemented (Phase 6)

## Context

Jobs may require a local model (project privacy "Local Models Only", §56, or an
agent routed to Ollama). Nodes differ in which models they have; downloading
the same multi-GB weights on every node from the internet is slow and may be
impossible on air-gapped machines. The coordinator should be able to push a
model it already has to selected nodes, verified end to end.

## Decision

1. **Content-addressed store.** Every node keeps blobs under
   `ORKESTR_HOME/blobs/sha256/<hex>` (flat; partial uploads in
   `blobs/staging/<hex>.part`, resumable); a blob is hash-verified (SHA-256)
   and atomically renamed on finalize. `Digest` =
   `sha256:<64 hex>` (`orkestr_domain::Digest`). The same store carries job
   checkpoints (ADR-0033).
2. **Model = manifest + layers.** `ModelManifest {name, tag, format,
   layers[{digest, size, mediaType}]}`; the manifest's own digest (SHA-256 of
   its canonical JSON) identifies the model. `models.import` adds a local GGUF
   (or a deterministic mock of a given size for tests) to the store;
   `models.list` lists it.
3. **Distribution over the node protocol.** `models.distribute {modelRef,
   nodeIds}` pushes each missing layer with `node.blobHas` (resume offset) →
   `node.blobPut` (512 KiB base64 chunks over E2E) and then calls
   `node.modelInstall {manifest, digest}`; the worker verifies every layer and
   the manifest digest before installing. A tampered chunk fails the finalize
   hash check and is discarded. `ModelDistributed` is emitted per node.
4. **Ollama adapter.** When `ollama` is present on the worker, an installed
   GGUF model is registered with `ollama create` (Modelfile `FROM <blob path>`);
   otherwise the model stays in the mock/local store only. Installed models
   appear in `NodeInfo.localModels` and in `NodeSnapshot.modelDigests`, which
   feed the scheduler's `requirements.localModels` filter.

## Consequences

- Transfers are resumable and verified; the gateway relays only ciphertext.
- Throughput is bounded by the E2E relay (no P2P); acceptable for the
  self-hosted single-owner case, revisit with LAN mTLS.
- Garbage collection of unreferenced blobs is deferred.
