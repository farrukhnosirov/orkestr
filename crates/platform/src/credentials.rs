//! OS credential store (§24): Windows Credential Manager, macOS Keychain,
//! Linux kernel keyutils (via the `keyring` crate).
//!
//! This is the low-level primitive; `orkestr-security::SecretStore` builds
//! scoping, auditing and the encrypted-file fallback on top. Errors never
//! contain secret values.

use std::collections::HashMap;

use parking_lot::Mutex;

use crate::error::{PlatformError, PlatformResult};

/// Key/value secret storage addressed by `(service, account)`.
pub trait CredentialStore: Send + Sync {
    /// Stores or replaces a secret.
    fn set(&self, service: &str, account: &str, secret: &str) -> PlatformResult<()>;
    /// Reads a secret (`None` when absent).
    fn get(&self, service: &str, account: &str) -> PlatformResult<Option<String>>;
    /// Deletes a secret. Returns whether it existed.
    fn delete(&self, service: &str, account: &str) -> PlatformResult<bool>;
}

fn validate(service: &str, account: &str) -> PlatformResult<()> {
    if service.trim().is_empty() || account.trim().is_empty() {
        return Err(PlatformError::Invalid("service and account must be non-empty".into()));
    }
    Ok(())
}

/// Native OS keyring.
#[derive(Debug, Default, Clone, Copy)]
pub struct KeyringCredentialStore;

impl KeyringCredentialStore {
    /// Creates the store.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

fn map_keyring(e: keyring::Error) -> PlatformError {
    // keyring errors never include the secret itself.
    PlatformError::Credential(e.to_string())
}

impl CredentialStore for KeyringCredentialStore {
    fn set(&self, service: &str, account: &str, secret: &str) -> PlatformResult<()> {
        validate(service, account)?;
        keyring::Entry::new(service, account).and_then(|e| e.set_password(secret)).map_err(map_keyring)
    }

    fn get(&self, service: &str, account: &str) -> PlatformResult<Option<String>> {
        validate(service, account)?;
        match keyring::Entry::new(service, account).and_then(|e| e.get_password()) {
            Ok(s) => Ok(Some(s)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(map_keyring(e)),
        }
    }

    fn delete(&self, service: &str, account: &str) -> PlatformResult<bool> {
        validate(service, account)?;
        match keyring::Entry::new(service, account).and_then(|e| e.delete_credential()) {
            Ok(()) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(e) => Err(map_keyring(e)),
        }
    }
}

/// In-memory store for tests and ephemeral cores. Never use for real secrets.
#[derive(Debug, Default)]
pub struct MemoryCredentialStore {
    entries: Mutex<HashMap<(String, String), String>>,
}

impl CredentialStore for MemoryCredentialStore {
    fn set(&self, service: &str, account: &str, secret: &str) -> PlatformResult<()> {
        validate(service, account)?;
        self.entries.lock().insert((service.into(), account.into()), secret.into());
        Ok(())
    }

    fn get(&self, service: &str, account: &str) -> PlatformResult<Option<String>> {
        validate(service, account)?;
        Ok(self.entries.lock().get(&(service.into(), account.into())).cloned())
    }

    fn delete(&self, service: &str, account: &str) -> PlatformResult<bool> {
        validate(service, account)?;
        Ok(self.entries.lock().remove(&(service.into(), account.into())).is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exercise(store: &dyn CredentialStore) {
        let service = "orkestr-test";
        let account = format!("acct-{}", std::process::id());
        let _ = store.delete(service, &account);
        assert_eq!(store.get(service, &account).unwrap(), None);
        store.set(service, &account, "value-1").unwrap();
        assert_eq!(store.get(service, &account).unwrap().as_deref(), Some("value-1"));
        store.set(service, &account, "value-2").unwrap();
        assert_eq!(store.get(service, &account).unwrap().as_deref(), Some("value-2"));
        assert!(store.delete(service, &account).unwrap());
        assert!(!store.delete(service, &account).unwrap());
        assert!(store.set("", "a", "x").is_err());
    }

    #[test]
    fn memory_store() {
        exercise(&MemoryCredentialStore::default());
    }

    /// Touches the real OS keyring; run explicitly with `--ignored`.
    #[test]
    #[ignore = "writes to the real OS credential store"]
    fn os_keyring_roundtrip() {
        exercise(&KeyringCredentialStore::new());
    }
}
