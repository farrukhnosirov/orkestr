# ADR-0031: Encrypted sync engine (Phase 4) — what is built and where it deviates from `sync-protocol.md`

- **Status:** Accepted
- **Date:** 2026-10-22
- **Phase:** 4 (Remote & Sync, package B)
- **Spec:** §40–45 (cloud workspace, Git ≠ live sync, continuous sync, offline-first, Seamless Continue, storage modes), §56 (No Remote Sync), §63 C/I
- **Builds on:** ADR-0019 (sync deferred behind `SyncBackend`), ADR-0030 (gateway mounts `/v1/sync`)
- **Docs:** `docs/sync/sync-protocol.md`

## Context

Phase 4 turns the `SyncBackend` seam into a working, end-to-end encrypted sync of
a project's working tree (including uncommitted files) and of small records
(UI state, task summary) between Core instances, through a server that must not
be able to read or forge anything. The full design in `sync-protocol.md` is
larger than one phase; this ADR records what was built and each deviation.

## Decision

`orkestr-sync` ships `DefaultSyncEngine` (implements the frozen `SyncEngine`
trait) and `server_router` (axum, mounted by the gateway at `/v1/sync`).

- **Journal / outbox / state** in `<data_dir>/sync/sync.db` (`sync_journal`,
  `sync_outbox`, `streams` base + last-seen version vector, `records`,
  `sync_conflicts`, file-hash cache). Sealed objects wait in `staging/` until
  uploaded; chunks are always uploaded before manifests, refs last (§8).
  A retry loop (30 s) re-runs rounds with pending outbox items or a failed last
  round, so work done offline is flushed when the server is back.
- **Chunking:** FastCDC 2020, normalization level 2, 16/64/256 KiB (§3.2).
- **Sealing:** zstd(3) → length prefix → zero padding to 4 KiB → XChaCha20-
  Poly1305, random 192-bit nonce, purpose-specific AAD (chunk id / stream id).
- **Keys:** `WDK = HKDF-SHA256(remote_key)`; `K_chunk`, `K_id`, `K_manifest`,
  `K_record`, `K_ring`, `K_stream` derived with HKDF info labels.
  Remote chunk ids are `keyed_blake3(K_id, plaintext)` (records:
  `keyed_blake3(K_id, "record\0" ‖ value)`); stream ids are
  `s_<keyed_blake3(K_stream, "workspace:"|"records:" ‖ remote project name)>`.
- **Signatures:** each device has an Ed25519 key; manifests (`id ‖ stream ‖
  version`) and refs (`stream ‖ manifest ‖ version ‖ epoch ‖ device ‖ ts`) are
  signed and verified against the **key ring**; unknown signer ⇒ `Integrity`.
- **Version vectors + anti-rollback:** a head whose vector is `Before` or
  `Concurrent` with the last seen one (or a vanished head) ⇒ `Integrity`.
- **CAS heads:** `PUT /heads/{stream}` with the expected ETag; 412 ⇒ fetch,
  merge, retry (≤ 4).
- **Merge:** per path against the last synced base: take the changed side;
  identical ⇒ nothing; modify beats delete; concurrent edits keep the local
  version in place and write the remote one to a **conflict copy**
  `name.conflict-<device>-<hash8>.ext` (it syncs too, so both versions exist on
  both devices) plus a `sync_conflicts` row. Records: last-writer-wins by HLC.
  First sync of a Git clone: files identical to `HEAD` (read-only `git ls-tree`,
  blob SHA-1 compare) count as unmodified, so remote edits apply without false
  conflicts.
- **Streams:** `workspace:` (scan honours `.gitignore`, `.orkestrignore`, hard
  excludes `.git/`, `target/`, `node_modules/`, OS junk, secret patterns, >512 MiB)
  and `records:` (`put_record` / `get_record`; Seamless Continue stores
  `uiState` and `taskSummary`).
- **Server:** filesystem object store `<dir>/<account>/{chunks,manifests}/..`,
  heads in SQLite `refs.db`, bearer token → account via the gateway's `AuthFn`.
  It sees opaque stream ids, sizes (padded), version vectors and device ids only.
- **Privacy:** `check_policy(effective_privacy)` — `No Remote Sync` (or
  classification `Secret`) ⇒ `PolicyDenied` ⇒ RPC `forbidden` with
  `data.syncError = "policyDenied"`, checked on configure / now / continueOn / resume.
- **Git:** sync never runs a Git command that writes; `.git/` is never read as
  data nor written by apply (paths are validated, excluded paths refused).
- **RPC:** `sync.configure` (generates and returns the remote key once when
  absent; optional `remoteProject`, default project slug; no `serverUrl` ⇒
  disable), `sync.status`, `sync.now`, `sync.continueOn {uiState}` (stores
  `uiState` + task summary built from `tasks.list` and `resume::summarize`, then
  syncs), `sync.resume` (sync + returns `uiState` and the summary),
  `sync.listConflicts`, `sync.resolveConflict` (keepLocal deletes the copy,
  keepRemote moves it over the file, keepBoth leaves it).

## Deviations from `sync-protocol.md`

1. **Key distribution:** all devices share a user-held `remote_key` (entered in
   Settings) instead of per-device HPKE envelopes of a random WDK. An
   X25519 + HKDF-SHA256 + XChaCha20-Poly1305 sealed-envelope primitive
   (`crypto::seal_envelope`, tested) replaces RFC 9180 HPKE for future use, but
   envelopes are not yet stored on the server. Recovery key / Argon2id: deferred.
2. **Key ring** is sealed with `K_ring` (any holder of the remote key may add
   itself) instead of a chain of member signatures. Rotation / revocation
   (`sync.rotateKeys`, epochs > 1): deferred, returns `unimplementedFeature`.
3. **Storage:** SQLite + filesystem behind the server module instead of
   PostgreSQL + S3; multipart upload sessions and `HEAD /chunks:batchHas` are
   replaced by `POST /chunks/has` and whole-object PUTs (objects ≤ 256 KiB,
   manifests ≤ 64 MiB body limit).
4. **Manifests:** JSON (zstd-compressed, sealed) instead of deterministic CBOR,
   one object per stream instead of Merkle-paged manifests.
5. **Merge:** conflict copies on disk instead of three-way text merge and a
   Monaco three-way UI; rename detection, case-collision and per-field record
   conflicts are not implemented. Conflict copies are visible to Git as
   untracked files until resolved.
6. **Git state descriptor** (§7: HEAD, unpushed-commit bundle, staged patch) is
   not synced; only the working tree is. Device B must already have the
   repository at the same commit for `git status` to match.
7. **Device secrets** (Ed25519 key, remote key, access token) live in the local
   `sync.db`, not the OS keystore / secrets vault.
8. No compression exception for `Secret` projects — they cannot sync at all.
9. Streams `worktree:`, `events:`, `snapshots:`, `artifacts:`, `records:global`
   and the editor `FileChanged{origin:"sync"}` notification are deferred.

## Consequences

- Seamless Continue (scenario C) works between Core instances that share a
  remote key and a gateway; nothing is lost on concurrent edits.
- The engine is the only code that knows the wire format; switching the
  server store to PostgreSQL/S3 changes `server.rs` only.
- Tests: version-vector laws (table-driven, all sample triples), FastCDC shift
  stability, sealing tamper, path validation, merge matrix, two engines + an
  in-process server (uncommitted + new + large files, records, ciphertext-only
  server, `git rev-list --count` unchanged, Git clone first sync), tampered
  chunk ⇒ `Integrity`, rollback and unknown signer rejected, offline outbox
  flush, policy denial; core `sync_rpc` test with two Cores over WebSocket RPC.
