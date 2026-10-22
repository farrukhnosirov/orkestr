# ADR-0036 — Web client over the E2E channel, and Web Push

Status: accepted (Phase 4, package C)

## Context

Phase 4 adds remote access through a self-hosted gateway (ADR-0030):
devices authenticate to the gateway with passkeys (TOTP fallback), pair with
a Core (codeMac + SAS, remote-access.md §4.2) and talk to it over the
end-to-end `e2e-v1` channel (§6) that the gateway relays as opaque bytes.
We need (1) a browser client that uses this channel without forking the
shared `@orkestr/client`, and (2) notifications to devices while the app is
closed (§11) without giving the gateway the plaintext.

## Decision

### Web client (`apps/web`, `@orkestr/web`)

- **RemoteTransport = `WebSocketFactory`.** `@orkestr/client` gets
  `createRemoteSocketFactory({gatewayUrl, coreId, deviceId, deviceKey,
  corePubKey})` (`packages/client/src/remote`). Its socket opens
  `/v1/connect?coreId=…`, runs the `e2e-v1` handshake in WebCrypto
  (P-256 ECDH/ECDSA, HKDF-SHA256, AES-256-GCM records with strictly
  sequential counters) and exposes plain text frames, so `OrkestrClient`
  (requests, subscriptions, reconnect) is unchanged. The client's `auth`
  frame travels encrypted and is ignored by the remote session; the
  principal comes from the handshake. Core refusals (`revoked`,
  `unknown_device`, bad CIK signature) close with 4401 (no reconnect);
  handshake timeouts close with 4408 (reconnect with backoff).
- The TS implementation is verified against the Core's published vector
  (`crates/core/src/remote/testdata/e2e-vector.json`, copied to
  `packages/client/src/remote/testdata`): transcript hash, ECDH, HKDF keys and
  records match byte-for-byte; ECDSA signatures are *verified* (Rust signs
  deterministically, WebCrypto does not).
- **Device key DK** is generated with `extractable: false` and persisted as a
  `CryptoKey` in IndexedDB (`orkestr-remote/devices`) behind a
  `DeviceKeyStore` interface (in-memory implementation for tests). The CIK
  is pinned at pairing (checked against the offer's fingerprint and the
  CIK-signed approval).
- **Step-up**: on `forbidden{stepUpRequired, challenge}` the UI asks for a
  WebAuthn assertion (`userVerification: "required"`) over that challenge
  and retries once with `params.stepUp`. The step-up credential is
  registered with the Core over E2E (`devices.registerStepUp`), never taken
  from the gateway.
- Scope: login, pairing, projects, tasks, approvals, remote terminal (xterm
  over `terminal.*`; shows "remote terminal disabled" on `forbidden`), a
  textarea editor over `files.read`/`files.write` (hash-guarded; Monaco is
  deferred to keep the bundle small), push toggle, en/ru. State is React
  state (no store needed at this size). Gateway URLs live in one module
  (`packages/client/src/remote/endpoints.ts`).

### Web Push

- **Encryption in the Core** (`orkestr-core::remote::push`): RFC 8291
  `aes128gcm` (ephemeral P-256 ECDH with the UA key, HKDF with the auth
  secret, single record, delimiter 0x02), tested with the RFC 8291 §5
  vector. The gateway only receives ciphertext through
  `RemoteService::send_push`.
- **Triggers**: `ApprovalRequested` (urgency high), `TaskStatusChanged` →
  `Completed` / `Blocked` / `Waiting`, `BuildFailed`,
  `TestRunFinished{failed>0}`, `BudgetThreshold`. Payload
  `{v:1, eventId, category, projectId?, ts, title, body}` (`PushPayload`);
  titles/bodies are generic (no file paths, prompts or secrets) because they
  appear on lock screens. `Topic` = base64url(SHA-256(collapseKey)[..24])
  (RFC 8030 limit of 32 chars).
- **Subscriptions** are owned by the calling device and stored by the Core
  in `<data>/push/subscriptions.json` (`push.subscribe/unsubscribe/test`;
  `push.vapidKey` returns the gateway's VAPID key once the tunnel reports
  it). Only https endpoints are accepted (http for loopback, for tests).
- **Delivery at the gateway** (`orkestr-gateway::push`): VAPID (RFC 8292)
  ES256 JWT `{aud: endpoint origin, exp: +12h, sub}`, header
  `Authorization: vapid t=…, k=…`, plus `TTL`, `Urgency`, `Topic`,
  `Content-Encoding: aes128gcm`. 404/410 → `Gone` (subscription should be
  dropped). The VAPID key is persisted at
  `<gateway-data>/push/vapid_private_key`.
- The service worker (`apps/web/public/sw.js`) shows `title`/`body` of the
  browser-decrypted payload and focuses/opens the app on click.

## Consequences

- One protocol client serves desktop (local WS) and web (E2E) unchanged.
- Delivery to real push services (FCM/Mozilla autopush/APNs web) cannot be
  verified offline; only the JWT/headers against a local mock endpoint and
  the RFC 8291 vector are tested. Native APNs/FCM and DND are P5.
- **Trust gap (threat model R-08):** the gateway serves the web client's
  JavaScript, so a compromised gateway could ship code that exfiltrates
  decrypted data or misuses the device key (non-extractable keys cannot be
  copied, but can be used while the page runs). E2E protects against a
  passive or relay-only gateway, not against a malicious one serving the
  app. Mitigations — signed/pinned builds (SRI manifest checked by a
  pinned PWA shell or the desktop app), installable PWA with update
  review — are **deferred**; until then the web client is as trustworthy as
  the gateway host. Desktop and future mobile apps are not affected.
- Lock-screen privacy per device (`generic` vs detailed), remaining panels
  and offline PWA caching are deferred.
