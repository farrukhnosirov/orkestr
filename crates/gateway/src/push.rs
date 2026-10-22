//! Web Push delivery (Phase 4, ADR-0036): VAPID (RFC 8292, ES256 JWT) and
//! the POST to the push service. Payloads arrive already encrypted by the
//! core (RFC 8291 `aes128gcm`); the gateway never sees plaintext.
//!
//! Step 0 skeleton: delivery is not implemented yet.

/// An encrypted push message to deliver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushDelivery {
    /// Push service endpoint.
    pub endpoint: String,
    /// RFC 8291 `aes128gcm` body.
    pub body: Vec<u8>,
    /// `Topic` header.
    pub topic: Option<String>,
    /// `Urgency` header.
    pub urgency: String,
    /// `TTL` header (seconds).
    pub ttl_secs: u32,
}

/// Push delivery errors.
#[derive(Debug, thiserror::Error)]
pub enum PushError {
    /// Not implemented yet (skeleton).
    #[error("web push delivery is not implemented yet (unimplemented-feature)")]
    Unimplemented,
}

/// Delivers `msg` to its push service.
///
/// # Errors
/// Always [`PushError::Unimplemented`] in the skeleton.
pub async fn deliver(msg: &PushDelivery) -> Result<(), PushError> {
    let _ = msg;
    Err(PushError::Unimplemented)
}
