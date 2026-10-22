# ADR-0032 — Mobile stack: Expo/React Native, pure-JS crypto provider, key storage, voice

Status: accepted, implementation Phase 5

## Context

Phase 5 adds a phone client for remote monitoring and control (emergency
stop, approvals, Manager chat, voice, quick review, light editing, push).
It must speak the same `e2e-v1` channel (ADR-0030) and RPC protocol as the
web client (ADR-0036) and reuse `@orkestr/client`, `@orkestr/protocol` and
the voice intent grammar instead of forking them. Constraints:

- Hermes (React Native's engine) has no `crypto.subtle` and no `TextDecoder`;
  `crypto.getRandomValues` is not built in either.
- The root npm workspace pins React 18 (desktop/web); current Expo SDKs
  require React 19.
- Windows is the primary dev host: Gradle/CMake (New Architecture) break on
  paths longer than MAX_PATH.

## Decision

1. **Stack.** Expo (SDK 58, React Native 0.88, React 19, Hermes, New
   Architecture) as a *dev build* (`expo-dev-client`, `expo prebuild`), not
   Expo Go. Android is built and verified in Phase 5; iOS shares the code
   but is unverified. Native folders (`android/`, `ios/`) are generated and
   not committed.
2. **Repository.** `apps/mobile` is a standalone npm project with its own
   `package-lock.json`, *outside* the root workspaces (root `workspaces` =
   `apps/desktop`, `apps/web`, `packages/*`). Metro watches `../../packages`
   and maps `@orkestr/client` / `@orkestr/protocol` to their `src`, while
   every other module (React) resolves only from `apps/mobile/node_modules`
   (`nodeModulesPaths`, `disableHierarchicalLookup`). TypeScript uses
   `paths` + `customConditions: ["source"]`; Jest uses `moduleNameMapper`.
3. **Crypto provider seam.** `@orkestr/client` gets a `RemoteCrypto`
   interface (`packages/client/src/remote/crypto.ts`): SHA-256, HMAC, HKDF,
   P-256 ECDSA verify, ECDH, AES-256-GCM, device signer. `webCrypto()` is
   the default; the mobile entry calls `setRemoteCrypto(nobleCrypto())`
   (`@noble/curves`, `@noble/hashes`, `@noble/ciphers`) after polyfilling
   `getRandomValues` (expo-crypto) and `TextDecoder`. Both providers must
   pass the same `e2e-vector.json` suite.
4. **Key storage.** The device key DK is a noble P-256 key whose raw scalar
   is kept in `expo-secure-store` (Android Keystore-wrapped / iOS Keychain),
   no user-auth requirement. A *separate* step-up key is stored with
   `requireAuthentication: true` (biometric-gated, ADR-0038).
5. **Voice.** The intent grammar moves to `packages/client/src/voice`
   (desktop keeps re-export shims). Mobile records with `expo-audio`, sends
   audio to the core's `voice.transcribe`, parses locally with the shared
   grammar, executes via RPC, and speaks replies with `expo-speech`. A
   "type instead" fallback is always available. On-device STT and a mobile
   wake word are deferred.

## Consequences

- **Gap vs non-exportable keys.** Unlike the browser's non-extractable
  WebCrypto key, the private scalar is decrypted into JS memory while
  signing; a compromised app process can exfiltrate it. True Keystore/Secure
  Enclave keys need a native module (stretch, not Phase 5).
- Two React versions in the repo; never hoist mobile deps into the root.
- Android builds must run from a short path (`subst O: …`); never build
  mobile inside agent worktrees.
- `expo-build-properties` enables cleartext HTTP so a dev build can reach a
  local gateway (`http://10.0.2.2:…`); release builds must use HTTPS.
