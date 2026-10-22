//! `remote.*` and `devices.*` — remote access through the self-hosted gateway
//! (Phase 4, ADR-0030): enabling the tunnel, pairing devices, step-up keys.
//!
//! Step 0 skeleton: types are minimal and may be extended (additively) by the
//! remote-access package; handlers currently return `unimplementedFeature`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{DeviceId, ProjectId, Timestamp};

use super::{method, Empty};
use crate::policy::Role;

/// Params of `remote.enable`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RemoteEnableParams {
    /// Gateway base URL (`https://…` or `http://127.0.0.1:8787`).
    pub gateway_url: String,
    /// One-time core enrollment token issued by the gateway.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub enrollment_token: Option<String>,
}

/// Remote-access state (result of `remote.enable` / `disable` / `status`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RemoteStatus {
    /// Remote access is enabled (tunnel is (re)dialed).
    pub enabled: bool,
    /// The tunnel is currently connected.
    pub connected: bool,
    /// Configured gateway URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub gateway_url: Option<String>,
    /// This core's id at the gateway.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub core_id: Option<String>,
    /// Open remote sessions.
    pub sessions: u32,
    /// Core identity key fingerprint (`base64url(SHA-256(SEC1 public key))`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub core_key_fingerprint: Option<String>,
    /// Last tunnel error (for the settings UI).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub last_error: Option<String>,
}

/// Result of `remote.revokeAllSessions`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RemoteRevokeAllResult {
    /// Sessions closed.
    pub revoked: u32,
}

/// Params of `devices.startPairing` (all optional; `{}` pairs a device).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DevicePairingStartParams {
    /// `"node"`: a coordinator core will claim this pairing to enroll this
    /// core as a worker node (Phase 6). Absent for ordinary devices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub kind: Option<String>,
    /// Trust level offered to the coordinator (`kind: "node"`; the approval may override it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub trust_level: Option<orkestr_domain::TrustLevel>,
}

/// Result of `devices.startPairing`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DevicePairingStart {
    /// Pairing id.
    pub pairing_id: String,
    /// Short code the user types on the new device.
    pub code: String,
    /// Expiry.
    pub expires_at: Timestamp,
    /// This core's id at the gateway.
    #[serde(default)]
    pub core_id: String,
    /// Gateway URL the device signs in to.
    #[serde(default)]
    pub gateway_url: String,
    /// Fingerprint of the core identity key (pinned by the device).
    #[serde(default)]
    pub core_key_fingerprint: String,
    /// QR payload (`orkestr-pair:` + JSON of the fields above).
    #[serde(default)]
    pub qr: String,
}

/// A pairing claimed by a device, waiting for local approval (SAS comparison).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PendingPairing {
    /// Pairing id.
    pub pairing_id: String,
    /// Device id assigned by the core.
    pub device_id: DeviceId,
    /// Name the device reported.
    pub device_name: String,
    /// Platform the device reported (`web`, `android`, …).
    pub platform: String,
    /// 6-digit short authentication string shown on both screens.
    pub sas: String,
    /// When the claim arrived.
    pub claimed_at: Timestamp,
    /// `"node"` when a coordinator core claims the pairing to enroll this core
    /// as a worker node (Phase 6); absent for ordinary devices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub kind: Option<String>,
}

/// A WebAuthn assertion used for step-up (all fields base64url).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StepUpAssertion {
    /// Credential id.
    pub credential_id: String,
    /// `authenticatorData`.
    pub authenticator_data: String,
    /// `clientDataJSON`.
    pub client_data_json: String,
    /// DER ECDSA signature.
    pub signature: String,
}

/// Payload of `RemoteSessionOpened` / `RemoteSessionRevoked`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RemoteSessionPayload {
    /// Device.
    pub device_id: DeviceId,
    /// Tunnel stream id.
    pub stream_id: u32,
    /// Reason (for revocations).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reason: Option<String>,
}

/// Payload of `DeviceAuthorized`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeviceAuthorizedPayload {
    /// The device record.
    pub device: DeviceInfo,
}

/// Payload of `DeviceRevoked`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeviceRevokedPayload {
    /// Device.
    pub device_id: DeviceId,
}

/// Params of `devices.approve`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeviceApproveParams {
    /// Pairing id from `devices.startPairing`.
    pub pairing_id: String,
    /// The user confirmed the SAS shown on both devices.
    pub sas_confirmed: bool,
    /// Granted role.
    pub role: Role,
    /// Allow remote terminal access.
    #[serde(default)]
    pub remote_terminal: bool,
    /// Restrict to these projects (`None` = all).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_ids: Option<Vec<ProjectId>>,
    /// Trust level granted to a coordinator (`kind: "node"` pairings, Phase 6;
    /// default `restricted`). Ignored for ordinary devices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub trust_level: Option<orkestr_domain::TrustLevel>,
}

/// A paired device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeviceInfo {
    /// Device id.
    pub device_id: DeviceId,
    /// User-visible name.
    pub name: String,
    /// Role.
    pub role: Role,
    /// Remote terminal allowed.
    pub remote_terminal: bool,
    /// Project restriction (`None` = all).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_ids: Option<Vec<ProjectId>>,
    /// Paired at.
    pub created_at: Timestamp,
    /// Last seen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub last_seen_at: Option<Timestamp>,
    /// Revoked.
    pub revoked: bool,
    /// Platform reported at pairing.
    #[serde(default)]
    pub platform: String,
    /// A step-up (WebAuthn) credential is registered.
    #[serde(default)]
    pub step_up_registered: bool,
    /// Step-up credential kind (`webauthn` | `native`) when registered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub step_up_kind: Option<String>,
    /// Trust level granted when this "device" is a coordinator core that
    /// enrolled this core as a worker node (Phase 6); absent for devices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub trust_level: Option<orkestr_domain::TrustLevel>,
}

/// Result of `devices.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeviceList {
    /// Devices.
    pub devices: Vec<DeviceInfo>,
    /// Claimed pairings waiting for `devices.approve`.
    #[serde(default)]
    pub pending: Vec<PendingPairing>,
}

/// Params of `devices.revoke`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeviceRevokeParams {
    /// Device to revoke.
    pub device_id: DeviceId,
}

/// Params of `devices.registerStepUp` (WebAuthn registration of a step-up key).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeviceRegisterStepUpParams {
    /// WebAuthn `PublicKeyCredential` (JSON form).
    pub credential: serde_json::Value,
}

method!(
    /// `remote.enable` (local only).
    Enable, "remote.enable", RemoteEnableParams, RemoteStatus
);
method!(
    /// `remote.disable` (kill switch).
    Disable, "remote.disable", Empty, RemoteStatus
);
method!(
    /// `remote.status`.
    Status, "remote.status", Empty, RemoteStatus
);
method!(
    /// `remote.revokeAllSessions`.
    RevokeAllSessions, "remote.revokeAllSessions", Empty, RemoteRevokeAllResult
);
method!(
    /// `devices.startPairing` (local only).
    DevicesStartPairing, "devices.startPairing", DevicePairingStartParams, DevicePairingStart
);
method!(
    /// `devices.approve` (local only).
    DevicesApprove, "devices.approve", DeviceApproveParams, DeviceInfo
);
method!(
    /// `devices.list`.
    DevicesList, "devices.list", Empty, DeviceList
);
method!(
    /// `devices.revoke`.
    DevicesRevoke, "devices.revoke", DeviceRevokeParams, Empty
);
method!(
    /// `devices.registerStepUp`.
    DevicesRegisterStepUp, "devices.registerStepUp", DeviceRegisterStepUpParams, Empty
);

/// Core ↔ gateway tunnel control frames (JSON text WebSocket messages on
/// `/v1/tunnel`; data frames are binary `sid_be32 ‖ bytes`). Not part of the
/// client RPC surface (no TS export).
pub mod tunnel {
    use serde::{Deserialize, Serialize};

    use crate::policy::Role;

    /// Domain separator of the tunnel hello signature:
    /// `Sign_CIK("orkestr-tunnel-v1" ‖ nonce)`.
    pub const HELLO_CONTEXT: &str = "orkestr-tunnel-v1";

    /// Gateway → core.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(tag = "t", rename_all = "camelCase")]
    pub enum ToCore {
        /// First frame: nonce the core signs with its CIK.
        Challenge {
            /// 32 random bytes, base64url.
            nonce: String,
        },
        /// Identity accepted (and enrollment completed).
        #[serde(rename_all = "camelCase")]
        Welcome {
            /// Core id.
            core_id: String,
            /// Owning account.
            account_id: String,
            /// Bearer token for gateway HTTP APIs (only on enrollment).
            #[serde(default, skip_serializing_if = "Option::is_none")]
            core_token: Option<String>,
            /// WebAuthn RP id.
            rp_id: String,
            /// WebAuthn origins.
            origins: Vec<String>,
            /// Gateway VAPID public key (base64url SEC1) for Web Push, when configured.
            #[serde(default, skip_serializing_if = "Option::is_none")]
            vapid_public_key: Option<String>,
            /// Native push transports the gateway can deliver (`fcm`, `apns`).
            #[serde(default, skip_serializing_if = "Vec::is_empty")]
            native_push: Vec<String>,
        },
        /// Fatal error; the gateway closes the tunnel.
        Error {
            /// Machine code (`unknown_core`, `bad_signature`, `bad_enrollment_token`, `superseded`).
            code: String,
            /// Text.
            message: String,
        },
        /// A client connected (`/v1/connect`); bytes for `sid` follow.
        #[serde(rename_all = "camelCase")]
        Open {
            /// Stream id.
            sid: u32,
            /// Client IP as seen by the gateway.
            #[serde(default, skip_serializing_if = "Option::is_none")]
            client_ip: Option<String>,
        },
        /// The client went away.
        Close {
            /// Stream id.
            sid: u32,
        },
        /// A device claimed a pairing (remote-access.md §4.2).
        #[serde(rename_all = "camelCase")]
        PairingClaim {
            /// Pairing id.
            pairing_id: String,
            /// Device key, SEC1 base64url.
            device_pub_key: String,
            /// Device name.
            device_name: String,
            /// Platform.
            platform: String,
            /// `HMAC-SHA256(code, devicePub ‖ pairingId)`, base64url.
            code_mac: String,
        },
    }

    /// Core → gateway.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(tag = "t", rename_all = "camelCase")]
    pub enum ToGateway {
        /// Reply to `challenge`.
        #[serde(rename_all = "camelCase")]
        Hello {
            /// Core id (absent on first enrollment).
            #[serde(default, skip_serializing_if = "Option::is_none")]
            core_id: Option<String>,
            /// CIK public key, SEC1 base64url.
            cik_public: String,
            /// One-time enrollment token (first connection only).
            #[serde(default, skip_serializing_if = "Option::is_none")]
            enrollment_token: Option<String>,
            /// Core version.
            version: String,
            /// `["e2e-v1","push-v1"]`.
            capabilities: Vec<String>,
            /// `Sign_CIK(HELLO_CONTEXT ‖ nonce)`, P1363 base64url.
            sig: String,
        },
        /// Close a client stream.
        Close {
            /// Stream id.
            sid: u32,
            /// Reason.
            reason: String,
        },
        /// Answer to `pairingClaim`.
        #[serde(rename_all = "camelCase")]
        PairingAck {
            /// Pairing id.
            pairing_id: String,
            /// Claim accepted (waiting for local approval).
            ok: bool,
            /// Device id assigned by the core.
            #[serde(default, skip_serializing_if = "Option::is_none")]
            device_id: Option<String>,
            /// Error code when `!ok`.
            #[serde(default, skip_serializing_if = "Option::is_none")]
            error: Option<String>,
        },
        /// Final pairing decision.
        #[serde(rename_all = "camelCase")]
        PairingResult {
            /// Pairing id.
            pairing_id: String,
            /// `approved` | `rejected` | `expired`.
            status: String,
            /// Device id.
            device_id: String,
            /// Granted role.
            #[serde(default, skip_serializing_if = "Option::is_none")]
            role: Option<Role>,
            /// `Sign_CIK(SHA-256("orkestr-pairing-v1" ‖ pairingId ‖ deviceId ‖ devicePub ‖ role))`.
            #[serde(default, skip_serializing_if = "Option::is_none")]
            sig: Option<String>,
        },
        /// Registry mirror update.
        #[serde(rename_all = "camelCase")]
        DeviceAuthorized {
            /// Device id.
            device_id: String,
            /// Name.
            name: String,
            /// Device key.
            public_key: String,
            /// Role.
            role: Role,
        },
        /// Device revoked: drop its sessions and push subscriptions.
        #[serde(rename_all = "camelCase")]
        RevokeDevice {
            /// Device id.
            device_id: String,
        },
        /// Web Push delivery request (body already RFC 8291 encrypted).
        #[serde(rename_all = "camelCase")]
        Push {
            /// Target devices.
            device_ids: Vec<String>,
            /// Push endpoint.
            endpoint: String,
            /// `aes128gcm` body, base64url.
            body: String,
            /// `Topic`.
            #[serde(default, skip_serializing_if = "Option::is_none")]
            collapse_key: Option<String>,
            /// `Urgency`.
            urgency: String,
            /// Delivery transport (`webPush` when absent; FCM/APNs: `endpoint` is the device token).
            #[serde(default)]
            transport: crate::methods::push::PushTransport,
        },
    }
}
