# ADR-0032 — Mobile stack: Expo/React Native, pure-JS crypto provider, key storage, voice

Status: accepted, implemented Phase 5 (Android verified on the emulator; iOS
unbuilt)

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

## As built (Phase 5)

- **Versions.** Expo SDK 58.0.17, React Native 0.88.0, React 19.3.0,
  TypeScript ~6.0, Hermes + New Architecture, `@noble/*` ^2.4, zustand 5,
  i18next 26 / react-i18next 17, jest-expo 58 + @testing-library/react-native
  14. `app.json`: scheme `orkestr`, `android.package` /
  `ios.bundleIdentifier` `dev.orkestr.mobile`; `app.config.js` adds
  `googleServicesFile` only when `apps/mobile/google-services.json` exists.
- **Layering.** `apps/mobile/src/core/` is pure TypeScript with no React
  Native imports (bundleable for Node): `gatewaySession.ts` (TOTP login,
  bearer token, `/v1/cores`), `connection.ts` (one `OrkestrClient` per paired
  core), `pairing.ts` (QR / `orkestr://pair?offer=` deep link / pasted text,
  SAS), `keys.ts` (device key + step-up key + counter), `approvals.ts`
  (`resolveWithStepUp`, `callWithStepUp`), `push.ts`, `monitoring.ts`,
  `voice/`. Platform access goes through interfaces (`SecureKV`,
  `BiometricGate`, `Notifier`, `Recorder`, `Speaker`) implemented with Expo
  modules in `src/adapters/expo.ts` and in memory for tests and the Node
  acceptance lane (`createMemoryCore`).
- **Screens.** Login, Cores, Pairing (SAS), Home (monitoring + Emergency
  Stop + remote kill switch), Approvals (biometric step-up), Manager (chat +
  microphone), Review (tasks, diff, transcript/logs), Editor (textarea with a
  hash guard), Devices (list + revoke), Settings (push categories, lock-screen
  privacy, en/ru, unpair). Every interactive element has a `testID`.
- **Crypto.** `index.ts` loads `src/polyfills.ts` (`getRandomValues` from
  `expo-crypto`, `TextDecoder`) and calls `setRemoteCrypto(nobleCrypto())`
  before registering the app. `e2e.ts`, `pairing.ts` and `randomBytes` in
  `@orkestr/client` go through `remoteCrypto()`; the shared
  `e2e-vector.json` suite runs over both providers, and under the noble
  provider `crypto.subtle` is replaced by a throwing proxy to prove no
  WebCrypto call remains.
- **Keys.** Device key: noble P-256 raw key in SecureStore (no user auth).
  Step-up key: separate P-256 key in a SecureStore item with
  `requireAuthentication: true`, registered with `devices.registerStepUp`
  right after pairing (`keyStorage` `android-keystore-wrapped` /
  `ios-keychain-wrapped`); its counter is kept in SecureStore (ADR-0038).
- **Voice.** `expo-audio` recording → core `voice.transcribe
  {audioBase64, mimeType}` → shared parser from `@orkestr/client` → RPC.
  "Stop all agents" asks for confirmation; "emergency stop" runs at once; a
  missing STT configuration surfaces as a named error, never as fake success.
  Replies are spoken with `expo-speech`; "type instead" is always available.
- **Notifications.** Native push registration via `push.registerNative`
  (ADR-0037) when the platform gives a device token; while connected the app
  also shows a local notification (`expo-notifications`) for E2E events such
  as `ApprovalRequested`, which works on an emulator without Firebase. A tap
  routes to the approval.
- **Deferred** (UI shows the localized `errors.unimplemented` text; core stubs
  return the unimplemented-feature error): passkey login on mobile (needs
  assetlinks / associated domains), on-device STT and a mobile wake word,
  background decryption for detailed lock-screen text and
  approve-from-notification, non-exportable hardware keys, DPoP,
  admin-device approval of new devices (`devices.approve` stays local-only),
  multi-node (`nodes.list` returns the local node; Phase 6), iOS build.

## Consequences

- **Gap vs non-exportable keys.** Unlike the browser's non-extractable
  WebCrypto key, the private scalar is decrypted into JS memory while
  signing; a compromised app process can exfiltrate it. SecureStore wraps it
  with a Keystore/Keychain key, and the step-up item is released only after
  the OS biometric prompt, but this is not a hardware non-exportable key.
  True Keystore/Secure Enclave signing keys need a native module (stretch,
  not built in Phase 5).
- Two React versions in the repo; never hoist mobile deps into the root, and
  run `npm install` / `typecheck` / `test` inside `apps/mobile`.
- Android builds run directly from `apps/mobile/android` with Windows long
  paths enabled and the SDK's ninja replaced by 1.12.1 (long-path aware).
  Building from a `subst` drive fails (React Native codegen rejects paths on
  different roots). Never build mobile inside agent worktrees.
- `expo-build-properties` enables cleartext HTTP so a dev build can reach a
  local gateway (`http://10.0.2.2:…` or `adb reverse`); release builds must
  use HTTPS.
- iOS shares all code but was not built: it needs macOS + Xcode.
