//! Credential resolution seam.
//!
//! Providers never persist keys themselves: the registry stores only a
//! [`SecretRefId`] per provider, and adapters resolve it at call time through
//! [`CredentialResolver`]. The core wires this to the security area's
//! `SecretStore` (OS keyring / encrypted vault).

use std::collections::HashMap;

use async_trait::async_trait;
use orkestr_domain::{ProviderId, SecretRefId};
use parking_lot::Mutex;

/// Credential failures (never contain secret values).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CredentialError {
    /// Reference unknown / secret deleted.
    #[error("credential not found")]
    NotFound,
    /// Vault locked (emergency control).
    #[error("credential vault is locked")]
    Locked,
    /// Backend failure.
    #[error("credential backend: {0}")]
    Backend(String),
}

/// Stores and resolves provider secrets by reference.
#[async_trait]
pub trait CredentialResolver: Send + Sync {
    /// Stores a secret for a provider and returns its reference.
    async fn store(&self, provider: ProviderId, label: &str, secret: &str) -> Result<SecretRefId, CredentialError>;
    /// Resolves a reference to the plaintext secret (audited by the backend).
    async fn resolve(&self, reference: SecretRefId) -> Result<String, CredentialError>;
    /// Deletes a secret. Missing secrets are not an error.
    async fn delete(&self, reference: SecretRefId) -> Result<(), CredentialError>;
}

/// In-memory resolver for tests and ephemeral cores. Never use for real keys.
#[derive(Debug, Default)]
pub struct MemoryCredentials {
    secrets: Mutex<HashMap<SecretRefId, String>>,
}

impl MemoryCredentials {
    /// Empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of stored secrets.
    #[must_use]
    pub fn len(&self) -> usize {
        self.secrets.lock().len()
    }

    /// Whether empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[async_trait]
impl CredentialResolver for MemoryCredentials {
    async fn store(&self, _provider: ProviderId, _label: &str, secret: &str) -> Result<SecretRefId, CredentialError> {
        let id = SecretRefId::new();
        self.secrets.lock().insert(id, secret.to_owned());
        Ok(id)
    }

    async fn resolve(&self, reference: SecretRefId) -> Result<String, CredentialError> {
        self.secrets.lock().get(&reference).cloned().ok_or(CredentialError::NotFound)
    }

    async fn delete(&self, reference: SecretRefId) -> Result<(), CredentialError> {
        self.secrets.lock().remove(&reference);
        Ok(())
    }
}
