//! Secret value backends (ADR-0011).
//!
//! * [`KeyringStore`] — the OS credential store via `orkestr-platform`
//!   (`keyring` crate: Windows Credential Manager, macOS Keychain, Linux
//!   keyutils). Entries: service `orkestr`, account `{dataDirHash8}/{locator}`.
//! * [`EncryptedFileVault`] — `<data_dir>/vault/secrets.bin`, every value
//!   sealed with XChaCha20-Poly1305 (random 24-byte nonce, AAD = locator).
//!   The 32-byte data key comes from the OS keyring (`{hash}/vault-key`) or,
//!   when no keyring is usable, from a key file written user-private (ACL'd
//!   to the current user on Windows). The vault file records which key
//!   source it was created with, so a flaky keyring can never silently
//!   create a second key and orphan the data.
//!
//! Values never appear in errors or logs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine as _;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use orkestr_platform::CredentialStore;
use parking_lot::Mutex;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use crate::error::{SecurityError, SecurityResult};

/// Keyring service name.
pub const KEYRING_SERVICE: &str = "orkestr";
/// Largest value stored in the keyring (Windows blob limit is 2560 bytes).
pub const MAX_KEYRING_VALUE: usize = 2048;
const VAULT_MAGIC: &str = "OKV1";

/// Short, stable hash of the data dir (keeps several `ORKESTR_HOME`s apart).
#[must_use]
pub fn data_dir_hash(data_dir: &Path) -> String {
    let h = blake3::hash(data_dir.to_string_lossy().as_bytes());
    h.to_hex()[..8].to_owned()
}

/// A value backend addressed by locator.
pub trait SecretStore: Send + Sync {
    /// Stores/replaces a value.
    fn put(&self, locator: &str, value: &str) -> SecurityResult<()>;
    /// Reads a value.
    fn get(&self, locator: &str) -> SecurityResult<Option<Zeroizing<String>>>;
    /// Deletes a value. Returns whether it existed.
    fn delete(&self, locator: &str) -> SecurityResult<bool>;
}

/// OS keyring backend.
pub struct KeyringStore {
    creds: Arc<dyn CredentialStore>,
    prefix: String,
}

impl std::fmt::Debug for KeyringStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyringStore").field("prefix", &self.prefix).finish_non_exhaustive()
    }
}

impl KeyringStore {
    /// Creates the store; `prefix` is the data-dir hash.
    #[must_use]
    pub fn new(creds: Arc<dyn CredentialStore>, prefix: impl Into<String>) -> Self {
        Self { creds, prefix: prefix.into() }
    }

    fn account(&self, locator: &str) -> String {
        format!("{}/{locator}", self.prefix)
    }
}

impl SecretStore for KeyringStore {
    fn put(&self, locator: &str, value: &str) -> SecurityResult<()> {
        if value.len() > MAX_KEYRING_VALUE {
            return Err(SecurityError::Backend(format!("value too large for the OS keyring (> {MAX_KEYRING_VALUE} bytes)")));
        }
        Ok(self.creds.set(KEYRING_SERVICE, &self.account(locator), value)?)
    }

    fn get(&self, locator: &str) -> SecurityResult<Option<Zeroizing<String>>> {
        Ok(self.creds.get(KEYRING_SERVICE, &self.account(locator))?.map(Zeroizing::new))
    }

    fn delete(&self, locator: &str) -> SecurityResult<bool> {
        Ok(self.creds.delete(KEYRING_SERVICE, &self.account(locator))?)
    }
}

/// Where the vault data key lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KeySource {
    /// OS keyring entry `{hash}/vault-key`.
    Keyring,
    /// User-private key file next to the vault.
    File,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct VaultFile {
    magic: String,
    version: u8,
    key_source: Option<KeySource>,
    entries: BTreeMap<String, VaultEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VaultEntry {
    /// base64 nonce
    n: String,
    /// base64 ciphertext
    c: String,
}

/// Encrypted file vault.
pub struct EncryptedFileVault {
    dir: PathBuf,
    creds: Option<Arc<dyn CredentialStore>>,
    prefix: String,
    key: Mutex<Option<Zeroizing<[u8; 32]>>>,
    io: Mutex<()>,
}

impl std::fmt::Debug for EncryptedFileVault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EncryptedFileVault").field("dir", &self.dir).field("unlocked", &self.key.lock().is_some()).finish()
    }
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    rand::rng().fill_bytes(&mut b);
    b
}

impl EncryptedFileVault {
    /// Vault in `dir` (`<data_dir>/vault`). `creds = None` forces a file key.
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>, creds: Option<Arc<dyn CredentialStore>>, prefix: impl Into<String>) -> Self {
        Self { dir: dir.into(), creds, prefix: prefix.into(), key: Mutex::new(None), io: Mutex::new(()) }
    }

    fn file(&self) -> PathBuf {
        self.dir.join("secrets.bin")
    }

    fn key_file(&self) -> PathBuf {
        self.dir.join("vault.key")
    }

    fn key_account(&self) -> String {
        format!("{}/vault-key", self.prefix)
    }

    fn read_file(&self) -> SecurityResult<VaultFile> {
        match std::fs::read(self.file()) {
            Ok(bytes) => {
                let f: VaultFile = serde_json::from_slice(&bytes).map_err(|e| SecurityError::Crypto(format!("vault file is corrupt: {e}")))?;
                if f.magic != VAULT_MAGIC {
                    return Err(SecurityError::Crypto("vault file has an unknown format".into()));
                }
                Ok(f)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Ok(VaultFile { magic: VAULT_MAGIC.into(), version: 1, key_source: None, entries: BTreeMap::new() })
            }
            Err(e) => Err(SecurityError::Backend(format!("cannot read vault: {e}"))),
        }
    }

    fn write_file(&self, f: &VaultFile) -> SecurityResult<()> {
        let bytes = serde_json::to_vec(f)?;
        orkestr_platform::fs::write_private_file(&self.file(), &bytes).map_err(|e| SecurityError::Backend(format!("cannot write vault: {e}")))
    }

    fn decode_key(hex_key: &str) -> SecurityResult<Zeroizing<[u8; 32]>> {
        let mut raw = Zeroizing::new(hex::decode(hex_key.trim()).map_err(|_| SecurityError::Crypto("vault key is malformed".into()))?);
        if raw.len() != 32 {
            return Err(SecurityError::Crypto("vault key has the wrong length".into()));
        }
        let mut k = Zeroizing::new([0u8; 32]);
        k.copy_from_slice(&raw);
        raw.zeroize();
        Ok(k)
    }

    /// Loads (or on first use creates) the data key. Idempotent.
    pub fn unlock(&self) -> SecurityResult<()> {
        let _io = self.io.lock();
        if self.key.lock().is_some() {
            return Ok(());
        }
        let mut file = self.read_file()?;
        let source = match file.key_source {
            Some(s) => s,
            None => {
                // First use: prefer the keyring, fall back to a key file.
                let fresh = Zeroizing::new(hex::encode(random_bytes::<32>()));
                let source = match &self.creds {
                    Some(c) => match c.set(KEYRING_SERVICE, &self.key_account(), &fresh) {
                        Ok(()) => KeySource::Keyring,
                        Err(e) => {
                            tracing::warn!(error = %e, "OS keyring unavailable for the vault key; using a user-private key file");
                            KeySource::File
                        }
                    },
                    None => KeySource::File,
                };
                if source == KeySource::File {
                    orkestr_platform::fs::write_private_file(&self.key_file(), fresh.as_bytes())
                        .map_err(|e| SecurityError::Backend(format!("cannot write vault key: {e}")))?;
                }
                file.key_source = Some(source);
                self.write_file(&file)?;
                source
            }
        };
        let hex_key: Zeroizing<String> = match source {
            KeySource::Keyring => {
                let c = self.creds.as_ref().ok_or_else(|| SecurityError::Backend("vault key is in the OS keyring but no keyring is configured".into()))?;
                Zeroizing::new(
                    c.get(KEYRING_SERVICE, &self.key_account())?
                        .ok_or_else(|| SecurityError::Crypto("vault key missing from the OS keyring".into()))?,
                )
            }
            KeySource::File => Zeroizing::new(
                std::fs::read_to_string(self.key_file()).map_err(|e| SecurityError::Backend(format!("cannot read vault key file: {e}")))?,
            ),
        };
        let key = Self::decode_key(&hex_key)?;
        *self.key.lock() = Some(key);
        Ok(())
    }

    /// Drops the cached data key (zeroized). Reads/writes fail with `VaultLocked`.
    pub fn lock(&self) {
        *self.key.lock() = None;
    }

    /// Whether the key is loaded.
    #[must_use]
    pub fn is_unlocked(&self) -> bool {
        self.key.lock().is_some()
    }

    fn cipher(&self) -> SecurityResult<XChaCha20Poly1305> {
        let guard = self.key.lock();
        let key = guard.as_ref().ok_or(SecurityError::VaultLocked)?;
        XChaCha20Poly1305::new_from_slice(key.as_slice()).map_err(|_| SecurityError::Crypto("invalid key".into()))
    }

    /// Encrypts arbitrary bytes with the vault key (AAD-bound).
    pub fn seal(&self, plain: &[u8], aad: &[u8]) -> SecurityResult<(Vec<u8>, [u8; 24])> {
        let cipher = self.cipher()?;
        let nonce = random_bytes::<24>();
        let ct = cipher
            .encrypt(XNonce::from_slice(&nonce), Payload { msg: plain, aad })
            .map_err(|_| SecurityError::Crypto("encryption failed".into()))?;
        Ok((ct, nonce))
    }

    /// Decrypts bytes sealed by [`seal`](Self::seal).
    pub fn open(&self, ct: &[u8], nonce: &[u8], aad: &[u8]) -> SecurityResult<Zeroizing<Vec<u8>>> {
        if nonce.len() != 24 {
            return Err(SecurityError::Crypto("bad nonce".into()));
        }
        let cipher = self.cipher()?;
        cipher
            .decrypt(XNonce::from_slice(nonce), Payload { msg: ct, aad })
            .map(Zeroizing::new)
            .map_err(|_| SecurityError::Crypto("decryption failed (wrong key or tampered vault)".into()))
    }
}

impl SecretStore for EncryptedFileVault {
    fn put(&self, locator: &str, value: &str) -> SecurityResult<()> {
        let (ct, nonce) = self.seal(value.as_bytes(), locator.as_bytes())?;
        let _io = self.io.lock();
        let mut f = self.read_file()?;
        let b64 = base64::engine::general_purpose::STANDARD;
        f.entries.insert(locator.to_owned(), VaultEntry { n: b64.encode(nonce), c: b64.encode(ct) });
        self.write_file(&f)
    }

    fn get(&self, locator: &str) -> SecurityResult<Option<Zeroizing<String>>> {
        if !self.is_unlocked() {
            return Err(SecurityError::VaultLocked);
        }
        let entry = {
            let _io = self.io.lock();
            self.read_file()?.entries.get(locator).cloned()
        };
        let Some(e) = entry else { return Ok(None) };
        let b64 = base64::engine::general_purpose::STANDARD;
        let nonce = b64.decode(&e.n).map_err(|_| SecurityError::Crypto("corrupt vault entry".into()))?;
        let ct = b64.decode(&e.c).map_err(|_| SecurityError::Crypto("corrupt vault entry".into()))?;
        let plain = self.open(&ct, &nonce, locator.as_bytes())?;
        let s = String::from_utf8(plain.to_vec()).map_err(|_| SecurityError::Crypto("vault value is not UTF-8".into()))?;
        Ok(Some(Zeroizing::new(s)))
    }

    fn delete(&self, locator: &str) -> SecurityResult<bool> {
        let _io = self.io.lock();
        let mut f = self.read_file()?;
        let existed = f.entries.remove(locator).is_some();
        if existed {
            self.write_file(&f)?;
        }
        Ok(existed)
    }
}

/// In-memory backend (tests / ephemeral cores). Never for real secrets.
#[derive(Debug, Default)]
pub struct MemorySecretStore {
    map: Mutex<BTreeMap<String, Zeroizing<String>>>,
}

impl SecretStore for MemorySecretStore {
    fn put(&self, locator: &str, value: &str) -> SecurityResult<()> {
        self.map.lock().insert(locator.to_owned(), Zeroizing::new(value.to_owned()));
        Ok(())
    }

    fn get(&self, locator: &str) -> SecurityResult<Option<Zeroizing<String>>> {
        Ok(self.map.lock().get(locator).cloned())
    }

    fn delete(&self, locator: &str) -> SecurityResult<bool> {
        Ok(self.map.lock().remove(locator).is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_platform::MemoryCredentialStore;

    #[test]
    fn vault_roundtrip_with_keyring_key() {
        let dir = tempfile::tempdir().unwrap();
        let creds: Arc<dyn CredentialStore> = Arc::new(MemoryCredentialStore::default());
        let v = EncryptedFileVault::new(dir.path().join("vault"), Some(Arc::clone(&creds)), "abcd1234");
        assert!(matches!(v.put("a", "x"), Err(SecurityError::VaultLocked)));
        v.unlock().unwrap();
        v.put("a", "super-secret-value-1").unwrap();
        v.put("b", "second").unwrap();
        assert_eq!(v.get("a").unwrap().as_deref().map(String::as_str), Some("super-secret-value-1"));
        assert!(v.get("missing").unwrap().is_none());
        let raw = std::fs::read_to_string(dir.path().join("vault/secrets.bin")).unwrap();
        assert!(!raw.contains("super-secret-value-1"), "values are encrypted at rest");
        assert!(!dir.path().join("vault/vault.key").exists(), "key lives in the keyring");
        v.lock();
        assert!(matches!(v.get("a"), Err(SecurityError::VaultLocked)));
        // A fresh instance (restart) unlocks with the same key.
        let v2 = EncryptedFileVault::new(dir.path().join("vault"), Some(creds), "abcd1234");
        v2.unlock().unwrap();
        assert_eq!(v2.get("b").unwrap().as_deref().map(String::as_str), Some("second"));
        assert!(v2.delete("b").unwrap());
        assert!(!v2.delete("b").unwrap());
    }

    #[test]
    fn vault_file_key_and_tamper_detection() {
        let dir = tempfile::tempdir().unwrap();
        let v = EncryptedFileVault::new(dir.path().join("vault"), None, "x");
        v.unlock().unwrap();
        v.put("loc", "value-123456").unwrap();
        assert!(dir.path().join("vault/vault.key").exists());
        // Swapping ciphertext between locators fails (AAD binding).
        let path = dir.path().join("vault/secrets.bin");
        let mut f: VaultFile = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let e = f.entries.get("loc").unwrap().clone();
        f.entries.insert("other".into(), e);
        std::fs::write(&path, serde_json::to_vec(&f).unwrap()).unwrap();
        assert!(matches!(v.get("other"), Err(SecurityError::Crypto(_))));
        assert_eq!(v.get("loc").unwrap().unwrap().as_str(), "value-123456");
        // Wrong key → decryption error, not garbage.
        std::fs::write(dir.path().join("vault/vault.key"), hex::encode([7u8; 32])).unwrap();
        let v2 = EncryptedFileVault::new(dir.path().join("vault"), None, "x");
        v2.unlock().unwrap();
        assert!(matches!(v2.get("loc"), Err(SecurityError::Crypto(_))));
    }

    #[test]
    fn keyring_store_uses_prefixed_accounts() {
        let creds = Arc::new(MemoryCredentialStore::default());
        let s = KeyringStore::new(creds.clone(), "pfx");
        s.put("id1", "v").unwrap();
        assert_eq!(creds.get(KEYRING_SERVICE, "pfx/id1").unwrap().as_deref(), Some("v"));
        assert!(s.put("big", &"x".repeat(MAX_KEYRING_VALUE + 1)).is_err());
        assert!(s.delete("id1").unwrap());
    }
}
