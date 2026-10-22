# ADR-0030 — Remote access: self-hosted gateway, `e2e-v1`, MethodPolicy enforcement

Status: accepted (Phase 4, package A)

## Context

`docs/architecture/remote-access.md` designs remote use of a Core from a
browser/phone without ever exposing a Core port: an outbound tunnel to a
gateway, a device ↔ Core end-to-end channel the gateway cannot read, explicit
device pairing, per-method authorization, step-up for dangerous actions,
revocation and a kill switch. Phase 4 builds the web half (push to native
apps is P5). The design assumed PostgreSQL, a gateway-run CA with mTLS and TLS
termination in the gateway; those are heavy for a self-hosted single-owner
deployment and for this phase.

## Decision

1. **New crate `orkestr-gateway`** (axum, lib + `orkestr-gateway` binary):
   account (single owner), passkeys, TOTP, sessions, core enrollment, tunnel
   relay, pairing relay, device-registry mirror, audit, `/v1/sync` (mounted
   from `orkestr-sync`), Web Push delivery, static web client (`--web-dist`).
2. **Storage: SQLite** (`<data-dir>/gateway.db`, rusqlite, WAL) behind one
   `store::Store` type. PostgreSQL is deferred (multi-instance deployments);
   rate limits are in-process (single instance).
3. **Core identity: CIK = ECDSA P-256** (`<data_dir>/remote/cik.key`, owner-only
   file) instead of Ed25519 + an X.509/mTLS client certificate. The tunnel
   (`/v1/tunnel`) authenticates the Core by a challenge: gateway sends a
   32-byte nonce, Core answers `Sign_CIK("orkestr-tunnel-v1" ‖ nonce)`; the
   first connection carries a one-time enrollment token (issued to a signed-in
   account, 15 min) and registers the CIK public key + issues a core bearer
   token (used for `/v1/sync`). The same key signs the E2E `serverHello` and
   the pairing completion, so devices pin one key (fingerprint in the QR).
4. **No TLS in the gateway.** It binds loopback by default and refuses a
   non-loopback bind unless `--behind-tls-proxy` (then cookies are `Secure`
   and `X-Forwarded-For` is trusted). The Core's tunnel dialer is built without
   TLS features (`ws://` to a loopback/LAN gateway); `wss://` needs a TLS
   build of the dialer (deferred).
5. **`e2e-v1`** exactly as the frozen wire spec (see
   `crates/core/src/remote/e2e.rs`, test vector `testdata/e2e-vector.json`):
   P-256 ECDH + ECDSA (P1363), transcript hash signed by both sides, HKDF-SHA256
   (`salt = nonceD ‖ nonceC`, `info = "orkestr-e2e-v1" ‖ coreId ‖ deviceId`) →
   `kD2C ‖ kC2D`, records `ctr_be64 ‖ AES-256-GCM(iv = dir_be32 ‖ ctr_be64,
   aad = ctr_be64)`, `dir` 1 = device→core, 2 = core→device, strict counters.
   Deviation from §6.2: AAD is the counter only (the gateway-assigned `sid` is
   not authenticated — it is not known to the device). Rekey: the session is
   closed after 2^20 records per direction; the 1 h re-handshake is not
   enforced yet.
6. **The client's `auth` frame is accepted on remote sessions and its token
   ignored** (deviation from §6.2 "no auth frame"): the principal comes from
   the handshake, and `OrkestrClient` keeps working unchanged over the
   remote transport.
7. **Enforcement in `RpcRouter::dispatch`**: `Principal::Local` is unrestricted;
   `Principal::Remote` goes through `remote::authz::authorize` — no policy ⇒
   forbidden, `local_only` ⇒ forbidden, role ceiling, `terminal.*` only with
   `remoteTerminal`, `projectId` param inside the device allowlist, per-device
   token buckets per `RateClass` (Read 50 rps, Write 10 rps, Expensive 1 rps
   burst 5, Auth 1/5 s burst 5, Emergency unlimited ⇒ `rate_limited
   {retryAfterMs}`), then step-up. Every remote decision is audited (method,
   params hash, decision). Subscriptions: topics filtered (`*` admin+unrestricted
   only, no `lsp:`, `terminal:` only with remote terminal, `project:` allowlist)
   plus an event-level filter. Stopping work (`emergency.*`, `agents.stop`,
   `remote.disable`, `remote.revokeAllSessions`) is allowed to every role.
8. **Step-up** (`StepUp::Always`, or `WhenDangerous` when the call is
   dangerous — approvals with risk ≥ High that are not rejections, and every
   destructive VCS/file/rollback method): the Core answers `forbidden
   {stepUpRequired, challenge}`; the challenge is single-use, 5 min, bound to
   device + method + params hash; the device retries with `params.stepUp =
   {credentialId, authenticatorData, clientDataJSON, signature}` (base64url,
   DER signature). The Core verifies against the credential registered with
   `devices.registerStepUp` inside the E2E channel (registration challenge not
   checked — the channel is already device-authenticated), RP id/origins as
   announced by the gateway at tunnel welcome. The Core never takes keys from
   the gateway at verification time.
9. **WebAuthn verifier is hand-written** (`orkestr_security::webauthn`): ES256
   only, attestation `none` only, UP+UV required, sign counter monotonic.
10. **Pairing** per §4.2 with `codeMac`, 6-digit SAS and a CIK-signed
    completion (`SHA-256("orkestr-pairing-v1" ‖ pairingId ‖ deviceId ‖ devicePub
    ‖ role)`); pending claims are surfaced in `devices.list.pending`; 5 wrong
    codeMacs burn the offer.
11. **Kill switch**: `remote.disable` persists `enabled=false`, closes all
    streams and the tunnel; the dialer never re-dials until a local
    `remote.enable`. `devices.revoke` works with the gateway down (Core refuses
    the handshake).

## Consequences

* Gateway compromise cannot read E2E traffic or forge device access: device
  keys and step-up credentials are Core-held; the integration test asserts the
  relay saw no plaintext.
* Deferred (explicit, RR-G14 where callable): PostgreSQL/multi-instance,
  ACME/TLS in the gateway and `wss://` dialing, mTLS/CA, device-code flow,
  admin-device approval of new devices, DPoP refresh tokens, 1 h rekey,
  per-device capability ceiling beyond role/terminal/projects, project
  scoping of methods that take only task/agent ids, account "panic" blocking
  re-dial (it drops tunnels and sessions; enrolled Cores reconnect).
* The gateway serves the web client from the same origin as the passkey RP,
  so the default RP id is `localhost` (`--rp-id` / `--origin` to change).
