//! Users and paired devices (§35–37, §55).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::common::{OsKind, Timestamp};
use crate::ids::{DeviceId, UserId};

/// A local user profile (§55 step 1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct User {
    /// Id.
    pub id: UserId,
    /// Display name.
    pub display_name: String,
    /// Optional e-mail (used for remote access/MFA later).
    pub email: Option<String>,
    /// Free-form UI/global preferences (validated by their owners).
    pub preferences: serde_json::Value,
    /// Creation time.
    pub created_at: Timestamp,
}

/// Kind of client device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum DeviceKind {
    /// Desktop app.
    Desktop,
    /// Browser (Web client).
    Web,
    /// iOS / Android app.
    Mobile,
    /// Headless node / CLI.
    Headless,
}

/// A device authorized to talk to this core (§36 device authorization).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Device {
    /// Id.
    pub id: DeviceId,
    /// Owner.
    pub user_id: UserId,
    /// Human readable name ("Pixel 9", "Work laptop").
    pub name: String,
    /// Device kind.
    pub kind: DeviceKind,
    /// OS of the device, if known.
    pub os: Option<OsKind>,
    /// Whether the device completed trust/pairing.
    pub trusted: bool,
    /// Revoked devices can no longer connect (§36 revoke).
    pub revoked: bool,
    /// Last time the device was seen.
    pub last_seen_at: Option<Timestamp>,
    /// Pairing time.
    pub created_at: Timestamp,
}
