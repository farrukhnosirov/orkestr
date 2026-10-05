//! `ui.previewPatch` / `ui.applyPatch` — validated UI config patches
//! (§29 AI-editable UI, ADR-0015, [`crate::ui_patch`]).
//!
//! * `ui.previewPatch` validates a patch against the stored configuration
//!   and returns the would-be document, the change list and the inverse
//!   patch — nothing is persisted. With an empty patch it simply returns the
//!   current document and version.
//! * `ui.applyPatch` validates again, stores a new version (optimistic
//!   concurrency through `baseVersion`) and publishes `UiConfigPatched`.
//!
//! Patches authored by the AI Manager reach the UI as `UiPatchProposed`
//! events; the user previews them and applies (or discards) them here.
//! `ui.getConfig` / `ui.setConfig` / `ui.history` stay with the theme engine.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::method;
use crate::ui_config::UiConfig;
use crate::ui_patch::{UiChange, UiPatchActor, UiPatchOp};

/// Params of `ui.previewPatch`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PreviewUiPatchParams {
    /// Operations (may be empty).
    #[serde(default)]
    pub patch: Vec<UiPatchOp>,
    /// Author whose allowlist applies (default `User`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub author: Option<UiPatchActor>,
}

/// Result of `ui.previewPatch`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PreviewUiPatchResult {
    /// The stored document.
    pub base: UiConfig,
    /// Its version (0 = never stored; defaults).
    pub version: u64,
    /// The document with the patch applied.
    pub config: UiConfig,
    /// What would change.
    pub changes: Vec<UiChange>,
    /// Patch that undoes the change.
    pub inverse: Vec<UiPatchOp>,
    /// Non-fatal notes.
    pub warnings: Vec<String>,
}

/// Params of `ui.applyPatch`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ApplyUiPatchParams {
    /// Operations (1..=64).
    pub patch: Vec<UiPatchOp>,
    /// Version the patch was previewed against; a different stored version → `conflict`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub base_version: Option<u64>,
    /// Author (default `User`). AI-authored patches get the narrow allowlist
    /// even when the user applies them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub author: Option<UiPatchActor>,
    /// One-line summary for the history (≤ 200 chars).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub summary: Option<String>,
    /// The `UiPatchProposed` proposal this applies, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub proposal_id: Option<String>,
}

/// Result of `ui.applyPatch`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ApplyUiPatchResult {
    /// The stored document.
    pub config: UiConfig,
    /// Its new version.
    pub version: u64,
    /// What changed.
    pub changes: Vec<UiChange>,
    /// Patch that undoes the change.
    pub inverse: Vec<UiPatchOp>,
    /// Non-fatal notes.
    pub warnings: Vec<String>,
}

method!(
    /// `ui.previewPatch`.
    PreviewPatch, "ui.previewPatch", PreviewUiPatchParams, PreviewUiPatchResult
);
method!(
    /// `ui.applyPatch`.
    ApplyPatch, "ui.applyPatch", ApplyUiPatchParams, ApplyUiPatchResult
);
