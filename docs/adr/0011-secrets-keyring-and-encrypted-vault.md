# ADR-0011: Secrets in the OS keyring, with an encrypted fallback vault

- **Status:** Accepted
- **Date:** 2026-10-02
- **Phase:** 1 (keyring + local vault), 4 (self-hosted/cloud secret providers)
- **Spec:** §13 (project secrets), §24, §50 (redaction), §52 (Lock Vault), §60.7, §64 (no plaintext secrets), §67 (no cross-project secret access)
- **Spike:** SP-18 · **Risk:** R-19

## Context

Orkestr stores provider API keys, environment secrets (DB URLs, deploy tokens) and later remote-access device keys. §24 lists the backends: Windows Credential Manager, macOS Keychain, Linux Secret Service, an encrypted local vault, and later self-hosted or cloud vaults. Contract §6 forbids plaintext in SQLite, logs and events.

## Decision drivers

1. Use OS-protected storage where it exists (DPAPI-backed Credential Manager, Keychain ACLs).
2. Work when the OS store is missing or limited: headless Linux without Secret Service, and Windows Credential Manager's 2560-byte blob limit.
3. Scoping and audit (§24): global, project and environment scopes, with every read audited.
4. Secrets never leave the core. The RPC can set and delete secrets but **never read** them.
5. Lock the vault on demand (§52).

## Considered options

| Option | Verdict |
|---|---|
| **`keyring` crate (v3) as primary + XChaCha20-Poly1305 file vault fallback, key in keyring or derived from a passphrase (Argon2id)** | **Chosen** |
| Encrypted SQLite (SQLCipher) for everything | Rejected. Key management problem is unchanged, and it complicates the bundled build |
| Vault file only, with a passphrase | Rejected as default. Prompting for a passphrase on every start hurts UX. Kept as the fallback when there is no keyring |
| Store in `.env` or config | Forbidden (§64) |

## Decision

```rust
#[derive(Clone, Debug, Serialize, Deserialize, TS)] #[serde(rename_all = "camelCase")]
pub struct SecretReference {
    pub id: SecretReferenceId,            // UUID v7
    pub scope: SecretScope,
    pub name: String,                     // e.g. "OPENAI_API_KEY"
    pub backend: SecretBackendKind,       // Keyring | Vault | (Phase 4) SelfHosted | Cloud
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub fingerprint: String,              // first 8 hex of blake3(value) — for "changed?" UI only, never reversible
}
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SecretScope { Global, Project { project_id: ProjectId }, Environment { project_id: ProjectId, environment_id: EnvironmentId }, Provider { provider_id: ProviderId } }

#[async_trait::async_trait]
pub trait SecretStore: Send + Sync + 'static {
    async fn put(&self, scope: SecretScope, name: &str, value: secrecy::SecretString, by: &Actor) -> Result<SecretReference, SecretError>;
    /// Only callable inside core by engines (providers, terminal env injection). Audited. Checks scope vs ctx.
    async fn resolve(&self, id: &SecretReferenceId, by: &Actor, ctx: &AccessContext) -> Result<secrecy::SecretString, SecretError>;
    async fn delete(&self, id: &SecretReferenceId, by: &Actor) -> Result<(), SecretError>;
    async fn list(&self, scope: Option<&SecretScope>) -> Result<Vec<SecretReference>, SecretError>;
    async fn lock(&self) -> Result<(), SecretError>;     // §52 Lock Vault: zeroize cached keys; resolve() → SecretError::Locked
    async fn unlock(&self, method: UnlockMethod) -> Result<(), SecretError>;
    fn status(&self) -> VaultStatus;                    // { backend, locked, keyringAvailable }
    /// Encrypt arbitrary blobs (e.g. snapshot copies of .env files, ADR-0021) with the vault data key. Fails when locked.
    async fn encrypt_blob(&self, plain: &[u8], aad: &[u8]) -> Result<Vec<u8>, SecretError>;
    async fn decrypt_blob(&self, sealed: &[u8], aad: &[u8]) -> Result<zeroize::Zeroizing<Vec<u8>>, SecretError>;
}
```

### Storage

- **Keyring entry:** service `orkestr`, user `"{dataDirHash8}/{secretRefId}"`. The data-dir hash keeps several `ORKESTR_HOME` instances (including tests) from colliding.
  - Values over 2048 bytes (below the Windows `CRED_MAX_CREDENTIAL_BLOB_SIZE` of 2560) go to the vault.
  - Tests never touch the real keyring: they use the `MemoryKeyring` backend through `ORKESTR_SECRET_BACKEND=memory`, which is refused in release builds.
- **Vault:** `<data_dir>/vault/secrets.bin`.
  - Format: `magic "OKV1" | version u8 | kdf u8 | salt[16] | entries…`. Each entry is XChaCha20-Poly1305 with a random 24-byte nonce. AAD = `secretRefId ‖ scope-json`, so ciphertext cannot be swapped between scopes.
  - The 32-byte data key is random and wrapped by:
    - (a) the keyring entry `orkestr/vault-key`; or
    - (b) Argon2id(passphrase, salt; m=64 MiB, t=3, p=1) when no keyring exists. In that case the vault starts **locked** and onboarding asks for the passphrase.
  - Writes are atomic: temp file, then fsync, then rename. The file is restricted to the user only.
- **SQLite** holds `secret_references` rows only (contract §6).

### Access control and audit

- `resolve` checks: a `Project` or `Environment` scope requires `ctx.project_id` to match. An agent actor additionally needs `PermissionEngine::check(.., Capability::Secrets, Resource::Secret{..})`.
- Provider keys (`Provider` scope) are resolved only by `orkestr-providers` with `Actor::System`, never by agents directly (§67).
- Every `resolve`, `put` and `delete` emits an audit row and a `SecretAccessed` event (CA-6) with the reference id and name only.
- **Injection into agent processes:** secrets are injected as env vars into a terminal or CLI agent only if they are listed in the task's environment binding. The values are registered with the redactor before the process starts.

### Redaction (`orkestr-security::redact`)

- A global `Redactor` combines:
  - an Aho-Corasick automaton over the values of currently resolved secrets (and their base64 and URL-encoded forms). Values are held in `Zeroizing` buffers and removed on lock;
  - regex detectors: `sk-[A-Za-z0-9_-]{20,}`, `sk-ant-…`, `AKIA[0-9A-Z]{16}`, `ghp_|gho_|github_pat_…`, `xox[abp]-…`, `AIza[0-9A-Za-z_-]{35}`, JWTs, `-----BEGIN [A-Z ]*PRIVATE KEY-----`, URL credentials `scheme://user:pass@`.
- It is applied in the `tracing` layer, the event publisher (payload string fields), `TerminalOutput` (agent-owned terminals; user-owned terminals have redaction on by default and the user can turn it off), and before **sending context to providers** (configurable; default on).

## Consequences

- **Positive:** OS-grade protection on all three platforms, works headless, auditable, Lock Vault is real.
- **Negative:** the keyring prompts on macOS the first time (Keychain ACL). This is expected and documented.
- **Negative:** the Linux Secret Service may be absent or locked (no D-Bus session in SSH or headless). The passphrase vault fallback covers it, and `orkestrd` needs an unlock (`orkestrd unlock` CLI or the first client to connect). SP-18 validates detection.
- **Negative:** redaction is best effort for unknown secret formats. Users should register secrets through Orkestr so the exact-value matcher sees them.

## Implementation notes (Phase 0 foundation, 2026-10-02)

Not implemented yet. As built: `orkestr-platform::CredentialStore { set, get, delete }` (synchronous, `&str` secrets) with `KeyringCredentialStore` (Windows Credential Manager, macOS Keychain, Linux kernel keyutils — **not** Secret Service, and not persistent across reboots) and `MemoryCredentialStore`; `orkestr-security::SecretStore { put(name, value) -> SecretReference, get(&SecretReference) -> String, delete }` is a skeleton trait. The domain types are `SecretReference`, `SecretRefId` (not `SecretReferenceId`), `SecretScope { Global | Project{projectId} | Environment{projectId, environmentId} }` (`tag = "scope"`; no `Provider` variant) and `SecretBackend { OsKeyring, EncryptedVault, SelfHostedVault, CloudSecretManager }`. Log redaction exists in `orkestr_core::redact` (registered RPC token, sensitive field names, well-known key formats). SP-18 has not run.
