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
}

/// Result of `remote.revokeAllSessions`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RemoteRevokeAllResult {
    /// Sessions closed.
    pub revoked: u32,
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
}

/// Result of `devices.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeviceList {
    /// Devices.
    pub devices: Vec<DeviceInfo>,
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
    DevicesStartPairing, "devices.startPairing", Empty, DevicePairingStart
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
