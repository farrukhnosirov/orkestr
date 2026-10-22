//! `push.*` — Web Push subscriptions (Phase 4, ADR-0036). Payloads are
//! encrypted in the core (RFC 8291) and delivered by the gateway (VAPID).
//!
//! Subscriptions are stored by the core; notifications are triggered by
//! contract events (`orkestr-core::remote::push`).

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

/// Decrypted Web Push payload (remote-access.md §11), shown by the web
/// client's service worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PushPayload {
    /// Payload version (1).
    pub v: u32,
    /// Source event id.
    pub event_id: String,
    /// Category (`approval`, `taskCompleted`, `taskBlocked`, `buildFailed`, `testFailed`, `budget`, `test`).
    pub category: String,
    /// Project scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<String>,
    /// RFC 3339 time.
    pub ts: String,
    /// Notification title.
    pub title: String,
    /// Notification body.
    pub body: String,
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
