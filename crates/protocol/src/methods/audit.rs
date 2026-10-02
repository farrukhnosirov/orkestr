//! `audit.*` — append-only audit log (§23, permission model §8).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{AuditEvent, ProjectId};

use super::method;

/// Params of `audit.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListAuditParams {
    /// Project filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
    /// Action prefix filter (`secrets.`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub action: Option<String>,
    /// Page backwards: rows with `seq < beforeSeq`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub before_seq: Option<i64>,
    /// Max rows (default 100, max 1000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<u32>,
    /// Also verify the hash chain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub verify: Option<bool>,
}

/// One audit row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AuditEntry {
    /// Monotonic sequence.
    pub seq: i64,
    /// Event.
    pub event: AuditEvent,
}

/// Result of `audit.list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListAuditResult {
    /// Entries, newest first.
    pub entries: Vec<AuditEntry>,
    /// With `verify`: whether the hash chain is intact.
    pub chain_valid: Option<bool>,
}

method!(
    /// `audit.list`.
    List, "audit.list", ListAuditParams, ListAuditResult
);
