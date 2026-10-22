# ADR-0038 — Native biometric step-up credential

Status: accepted, implementation Phase 5

## Context

Dangerous remote actions (approvals of dangerous requests, `devices.revoke`,
…) require a fresh step-up (ADR-0030, remote-access.md §6.3). The web client
uses a WebAuthn assertion verified in the core by
`webauthn::verify_assertion_json` (rpId hash, origin, UP+UV flags,
increasing counter). Platform passkeys in a React Native app need
Digital Asset Links / Associated Domains bound to the gateway domain, which
self-hosted gateways usually cannot provide.

## Decision

1. A **native step-up credential** of type `orkestr-native-v1`: a P-256 key
   generated on the phone and stored in SecureStore with
   `requireAuthentication: true` (biometric/device-credential gated),
   registered via `devices.registerStepUp` with
   `{ type, credentialId, publicKey, keyStorage }`.
2. The phone produces a **WebAuthn-shaped assertion** so the core reuses the
   existing verifier unchanged: `authenticatorData = SHA-256(rpId) ‖ 0x05
   (UP|UV) ‖ counter_be32`, `clientDataJSON = {type:"webauthn.get",
   challenge, origin, crossOrigin:false}`, signature (DER) over
   `authenticatorData ‖ SHA-256(clientDataJSON)`, with
   `rpId = "orkestr-native-stepup"` and
   `origin = "orkestr-native:<deviceId>"`. The counter is persisted in
   SecureStore and must strictly increase.
3. The core stores `StepUpKind::Native` on the device record and verifies
   native assertions against the per-device relying party above; WebAuthn
   credentials keep working. `DeviceInfo.stepUpKind` exposes the kind.
4. If the biometric prompt is denied the call is not retried and the
   approval stays pending.

## Consequences

- The origin binds an assertion to one device: another device's credential
  or a replayed/stale counter is rejected.
- UV is self-asserted by the app (the OS gate is what enforces it); the key
  is SecureStore-wrapped, not a non-exportable hardware key — same gap as
  ADR-0032. A native Keystore/Secure Enclave module is the stretch goal.
- Passkey login on mobile stays disabled (unimplemented) until
  assetlinks/associated domains are configured.
