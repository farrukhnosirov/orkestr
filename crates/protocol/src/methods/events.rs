//! `events.*` — event log replay (§21 Replay, reconnect gap filling).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::event::Event;

use super::method;

/// Maximum events returned by one `events.replay` call.
pub const MAX_REPLAY_LIMIT: u32 = 1000;

/// Params of `events.replay`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReplayParams {
    /// First sequence number (inclusive). Default 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub from_seq: Option<u64>,
    /// Last sequence number (inclusive). Default: latest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub to_seq: Option<u64>,
    /// Max events (default and cap: 1000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<u32>,
    /// Only events matching these topics (same rules as `sub`). Default: all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub topics: Option<Vec<String>>,
    /// Return the newest `limit` events in the range instead of the oldest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub newest_first: Option<bool>,
}

/// Result of `events.replay`. Events are always in ascending `seq` order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReplayResult {
    /// Events in ascending `seq` order.
    pub events: Vec<Event>,
    /// Latest persisted sequence number.
    pub last_seq: u64,
    /// True when more events exist in the requested range.
    pub has_more: bool,
}

method!(
    /// `events.replay`.
    Replay, "events.replay", ReplayParams, ReplayResult
);
