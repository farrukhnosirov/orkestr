# ADR-0038 — Native biometric step-up credential

Status: accepted, implemented Phase 5 (WP-A core side)

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
   registered over E2E via `devices.registerStepUp` with
   `credential = { type: "orkestr-native-v1", credentialId (b64url, 1–1023
   bytes), publicKey (SEC1 P-256, b64url; compressed accepted, stored
   uncompressed), keyStorage: "android-keystore-wrapped" |
   "ios-keychain-wrapped" | "software" }`. `keyStorage` is recorded in the
   core audit entry.
2. The phone produces a **WebAuthn-shaped assertion** so the core reuses the
   existing verifier unchanged:
   - `authenticatorData = SHA-256(rpId) ‖ 0x05 (UP|UV) ‖ counter_be32`;
   - `clientDataJSON = {"type":"webauthn.get","challenge":<b64url>,"origin":<origin>,"crossOrigin":false}`
     (byte order of keys as written; the verifier parses JSON, so only the
     signature binds the exact bytes);
   - `signature` = ECDSA P-256/SHA-256 over
     `authenticatorData ‖ SHA-256(clientDataJSON)`, **DER-encoded** like
     WebAuthn (the verifier also accepts raw 64-byte `r ‖ s`);
   - `rpId = "orkestr-native-stepup"`, `origin = "orkestr-native:<deviceId>"`;
   - the flat assertion JSON `{credentialId, authenticatorData,
     clientDataJSON, signature}` goes in `params.stepUp`, exactly as for
     WebAuthn.
   Reference: `crates/core/src/remote/native_stepup.rs::build_assertion` and
   `crates/core/src/remote/testdata/native-stepup-vector.json` (`privateKey`,
   `publicKey`, `deviceId`, `credentialId`, `rpId`, `origin`, `challenge`,
   `counter`, `registration`, `clientDataJSONText`, `authenticatorData`,
   `clientDataJSON`, `signature` (DER), `signatureRaw` (r‖s)). Signatures are
   RFC 6979 deterministic with low-S, so a conforming client reproduces the
   vector byte for byte.
3. The core stores `StepUpKind::Native` on the device record
   (`stepUpKind`, `#[serde(default)]` = `webauthn` for Phase 4 records) and
   verifies native assertions against the per-device relying party above;
   WebAuthn credentials keep working against the gateway RP.
   `DeviceInfo.stepUpKind` is `"native"` or `"webauthn"`. Registering a new
   credential of either kind replaces the previous one.
4. The **counter must strictly increase, including from 0** (the core checks
   this before the shared verifier, which tolerates `0 → 0` for counterless
   authenticators). The phone persists it in SecureStore.
5. If the biometric prompt is denied the call is not retried and the
   approval stays pending (client side, WP-C).

## Consequences

- The origin binds an assertion to one device: another device's credential,
  an assertion built for another device's origin, a replayed challenge and a
  stale counter are all rejected (unit tests in `native_stepup.rs`, E2E in
  `crates/gateway/tests/remote_e2e.rs`).
- UV is self-asserted by the app (the OS gate is what enforces it); the key
  is SecureStore-wrapped, not a non-exportable hardware key — same gap as
  ADR-0032. A native Keystore/Secure Enclave module is the stretch goal.
- Passkey login on mobile stays disabled (unimplemented) until
  assetlinks/associated domains are configured.
