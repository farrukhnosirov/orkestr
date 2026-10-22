# ADR-0037 — Native push: FCM HTTP v1 and APNs with end-to-end encrypted payloads

Status: accepted, implementation Phase 5

## Context

Phase 4 delivers notifications via Web Push (ADR-0036): the core encrypts
the payload (RFC 8291) and the gateway only relays ciphertext. Phones need
FCM (Android) and APNs (iOS). Those services see whatever is in the
notification, and a killed app cannot decrypt before the OS shows an alert.
The gateway is untrusted for content (ADR-0030).

## Decision

1. **Registration.** New method `push.registerNative`
   (`PushRegisterNativeParams { transport: fcm|apns, token, key, categories?,
   lockScreen: generic|detailed }` → `PushNativeRegistration { id,
   deliverable }`), policy Viewer / no step-up / Write rate class. `key` is a
   device-generated 32-byte AES-256 key (base64url) stored only in the core
   with the subscription; `deliverable` reports whether the gateway
   announced a sender for that transport (`ToCore::Welcome.nativePush`).
2. **Payload.** The core serializes the existing `PushPayload`, encrypts it
   with AES-256-GCM (random 12-byte nonce, AAD `"orkestr-push-v1"`) and
   sends `{"n": b64u(nonce), "c": b64u(ciphertext)}` through the tunnel as
   `ToGateway::Push` with `transport: fcm|apns` (the token in `endpoint`).
   A reference vector lives in `crates/core/src/remote/testdata`.
3. **Delivery (gateway).** FCM HTTP v1: service-account JSON, RS256 JWT →
   OAuth token (cached), `POST /v1/projects/{id}/messages:send`, Android
   priority high, `collapse_key`, data `{p, k}`. APNs: ES256 provider JWT,
   HTTP/2, `apns-push-type: alert`, `apns-collapse-id`, priority 10,
   `mutable-content: 1`. Endpoints are overridable for mocks; `Gone`
   (`UNREGISTERED`/404/410) drops the registration.
4. **Visible alert is generic** ("Orkestr — Action needed"). The app
   decrypts `p` when it runs (tap / foreground) and routes to the item.
   While connected, the app also raises a local notification from the E2E
   event stream, which works without Firebase (emulator).
5. Quiet hours / DND skip non-high-priority pushes; approvals still go.

## Consequences

- FCM/APNs learn only timing, size and category (`k`); content stays E2E.
- `lockScreen: detailed` and approve-from-notification need background
  decryption (Notification Service Extension / FCM data handler) — deferred;
  Phase 5 always shows the generic alert.
- Real delivery requires the operator's Firebase project / Apple key; CI and
  acceptance use mock FCM/APNs servers.
