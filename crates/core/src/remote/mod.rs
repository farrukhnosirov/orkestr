//! Remote access (Phase 4, ADR-0030): gateway tunnel, E2E sessions, device
//! registry, step-up, kill switch, Web Push delivery.
//!
//! Step 0 skeleton: [`RemoteService`] exists so `Services.remote` and the
//! push triggers can be wired; every operation returns `unimplementedFeature`
//! until the remote-access package lands.

use orkestr_domain::DeviceId;
use orkestr_protocol::RpcError;

/// A Web Push message, already encrypted in the core (RFC 8291 `aes128gcm`),
/// handed to the gateway for VAPID signing + delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushRequest {
    /// Target devices (subscriptions owned by these devices).
    pub device_ids: Vec<DeviceId>,
    /// Push service endpoint.
    pub endpoint: String,
    /// RFC 8291 `aes128gcm` body.
    pub body: Vec<u8>,
    /// `Topic` header (collapse key).
    pub collapse_key: Option<String>,
    /// `Urgency` header (`very-low` | `low` | `normal` | `high`).
    pub urgency: String,
}

/// Remote-access service.
#[derive(Debug, Default)]
pub struct RemoteService {
    _private: (),
}

impl RemoteService {
    /// Disabled remote service (no gateway configured).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sends an encrypted push message through the gateway.
    ///
    /// # Errors
    /// Always `unimplementedFeature` in the skeleton.
    pub async fn send_push(&self, req: PushRequest) -> Result<(), RpcError> {
        let _ = req;
        Err(RpcError::unimplemented_feature("remote.push"))
    }
}
