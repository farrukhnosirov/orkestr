//! `push.*` — Web Push subscriptions (Phase 4, ADR-0036). Payloads are
//! encrypted in the core (RFC 8291) and delivered by the gateway (VAPID).
//!
//! Step 0 skeleton: types are minimal and may be extended (additively) by the
//! push package; handlers currently return `unimplementedFeature`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{method, Empty};

/// Result of `push.vapidKey`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PushVapidKey {
    /// VAPID application server public key (SEC1 uncompressed, base64url).
    pub public_key: String,
}

/// Params of `push.subscribe` (a browser `PushSubscription`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PushSubscribeParams {
    /// Push service endpoint.
    pub endpoint: String,
    /// UA public key `p256dh` (base64url).
    pub p256dh: String,
    /// UA auth secret (base64url).
    pub auth: String,
    /// Notification categories (`approval`, `taskCompleted`, …); `None` = all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub categories: Option<Vec<String>>,
}

/// Result of `push.subscribe`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PushSubscription {
    /// Subscription id.
    pub id: String,
}

/// Params of `push.unsubscribe`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PushUnsubscribeParams {
    /// Push service endpoint.
    pub endpoint: String,
}

/// Result of `push.test`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PushTestResult {
    /// Messages handed to the gateway.
    pub sent: u32,
}

method!(
    /// `push.vapidKey`.
    VapidKey, "push.vapidKey", Empty, PushVapidKey
);
method!(
    /// `push.subscribe`.
    Subscribe, "push.subscribe", PushSubscribeParams, PushSubscription
);
method!(
    /// `push.unsubscribe`.
    Unsubscribe, "push.unsubscribe", PushUnsubscribeParams, Empty
);
method!(
    /// `push.test`.
    Test, "push.test", Empty, PushTestResult
);
