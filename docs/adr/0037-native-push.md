# ADR-0037 — Native push: FCM HTTP v1 and APNs with end-to-end encrypted payloads

Status: accepted, implemented Phase 5 (core + gateway + mobile client;
verified against mock FCM/APNs only)

## Context

Phase 4 delivers notifications via Web Push (ADR-0036): the core encrypts
the payload (RFC 8291) and the gateway only relays ciphertext. Phones need
FCM (Android) and APNs (iOS). Those services see whatever is in the
notification, and a killed app cannot decrypt before the OS shows an alert.
The gateway is untrusted for content (ADR-0030).

## Decision

1. **Registration.** `push.registerNative`
   (`PushRegisterNativeParams { transport: fcm|apns, token, key, categories?,
   lockScreen: generic|detailed }` → `PushNativeRegistration { id,
   deliverable }`), policy Viewer / no step-up / Write rate class. `key` is a
   device-generated 32-byte AES-256 key (base64url) stored only in the core
   (`<data>/push/subscriptions.json`, `StoredSubscription { transport, key,
   lockScreen }`, all `#[serde(default)]` so Phase 4 files still load).
   `transport: webPush` is rejected (use `push.subscribe`). A device has at
   most one registration per transport: a new token replaces the old one
   (same id). `deliverable` is true iff the gateway listed the transport in
   `ToCore::Welcome.nativePush` on the last tunnel connect (persisted in
   `remote/state.json`).
2. **Payload.** The core serializes the existing `PushPayload` and encrypts
   it with AES-256-GCM: random 12-byte nonce, AAD `"orkestr-push-v1"`,
   `c = ciphertext ‖ 16-byte tag`, giving `p = {"n": b64u(nonce), "c":
   b64u(c)}`. The core hands the gateway the envelope
   `{"k": category, "p": p}` as `ToGateway::Push { transport: fcm|apns,
   endpoint: <device token>, body: b64u(envelope JSON), collapseKey:
   SHA-256-derived topic, urgency }` (no protocol change beyond Step 0).
   Reference vector: `crates/core/src/remote/testdata/native-push-vector.json`
   (`key`, `nonce`, `aad`, `plaintext`, `payload`, `body.{n,c}`).
3. **Delivery (gateway, `crates/gateway/src/push_native.rs`).**
   - FCM HTTP v1 (`--fcm-service-account <json>`, `--fcm-endpoint`, default
     `https://fcm.googleapis.com`): RS256 JWT (`iss` = `client_email`, `scope`
     `firebase.messaging`, `aud` = `token_uri`, 1 h) via the `rsa` crate,
     JWT-bearer exchange at the service account's `token_uri` (cached until
     `expires_in − 60 s`, dropped on 401), then
     `POST /v1/projects/{project_id}/messages:send` with
     `android.priority`, `android.collapse_key`, generic
     `notification {title: "Orkestr", body: "Action needed"}` and
     `data {p: <JSON text of p>, k: category}`. 404 or `UNREGISTERED` → Gone;
     anything else → retryable.
   - APNs (`--apns-key <.p8> --apns-key-id --apns-team-id --apns-topic`,
     `--apns-endpoint`, default `https://api.push.apple.com`): ES256
     provider JWT (`kid`, `iss`, `iat`; cached 50 min, dropped on
     `ExpiredProviderToken`), HTTP/2 (`http://` endpoints use prior-knowledge
     h2c for mocks), `POST /3/device/{token}` with `apns-push-type: alert`,
     `apns-priority`, `apns-collapse-id`, `apns-topic`; body
     `{aps: {alert: {title, body}, mutable-content: 1, sound}, p, k}`.
     410 / `Unregistered` / `BadDeviceToken` → Gone; else retryable.
   - `ToCore::Welcome.nativePush` lists the configured transports; a push for
     an unconfigured transport is audited as `NotConfigured`.
4. **Visible alert is generic** ("Orkestr — Action needed"). The app
   decrypts `p` when it runs (tap / foreground) and routes to the item.
   While connected, the app also raises a local notification from the E2E
   event stream, which works without Firebase (emulator).
5. **Quiet hours / DND.** Non-high pushes (everything except approvals) are
   skipped while the global `doNotDisturb` setting is on or inside the
   notification quiet hours (`voice` config `notifications.quietHours`, local
   time, may wrap midnight). Approvals (`urgency: high`) always go.
   `push.test` ignores DND.

## Deviations from the Step 0 sketch

- Priority follows urgency: `high` (approvals) → FCM `HIGH` / APNs 10;
  others → `NORMAL` / 5 (FCM deprioritizes apps that send non-user-visible
  high-priority messages; Apple recommends 5 for non-urgent alerts).
- `p` is carried as JSON *text* in both FCM `data` (values must be strings)
  and the APNs payload, so one client decoder handles both.
- Gone tokens are audited at the gateway (`push.deliver` with `gone: true`)
  but not yet reported back to the core; the registration stays until the
  device re-registers, unregisters, or is revoked (revocation drops all its
  registrations).

## Client side (Phase 5, `apps/mobile`)

- After pairing, the app asks `expo-notifications` for the device push token
  (`getDevicePushTokenAsync`; FCM on Android, APNs on iOS), generates the
  32-byte key, keeps it in SecureStore and calls `push.registerNative` with
  the categories and lock-screen choice from Settings. Without
  `google-services.json` (emulator, CI) there is no token; registration is
  skipped and only local notifications are used.
- `@orkestr/client` `decryptNativePush` (`packages/client/src/remote/nativePush.ts`)
  opens `p` with the vector shared with the core
  (`packages/client/src/remote/testdata`, byte-identical to the Rust one).
  The app accepts either the envelope `{p}` or a bare `{n, c}`; tapping a
  notification decrypts and routes to the approval
  (`routeForNotification`).
- Lock-screen privacy `detailed` is stored and sent, but the alert is still
  generic until background decryption exists (see Consequences).

## Consequences

- FCM/APNs learn only timing, size, collapse topic and category (`k`);
  content stays E2E.
- `lockScreen: detailed` and approve-from-notification need background
  decryption (Notification Service Extension / FCM data handler) — deferred;
  Phase 5 always shows the generic alert.
- Real delivery requires the operator's Firebase project / Apple key; CI and
  acceptance use mock FCM/APNs servers (axum mocks in
  `push_native.rs` tests and `tests/remote_e2e.rs`).
