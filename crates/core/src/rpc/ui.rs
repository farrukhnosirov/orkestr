//! `ui.previewPatch` / `ui.applyPatch` handlers (§29 AI-editable UI,
//! ADR-0015). Validation is [`orkestr_protocol::ui_patch::apply_ui_patch`];
//! the core is the only trusted validator.
//!
//! Applying a patch that changes `performanceMode` also mirrors it into the
//! application settings (`AppSettings.performanceMode`, the value the UI's
//! performance store follows) and publishes `SettingsChanged`.

use orkestr_protocol::event::{EventKind, SettingsChangedPayload, UiConfigPatchedPayload};
use orkestr_protocol::methods::ui::{
    self as m, ApplyUiPatchParams, ApplyUiPatchResult, PreviewUiPatchParams, PreviewUiPatchResult,
};
use orkestr_protocol::ui_config::UiConfigIssue;
use orkestr_protocol::ui_patch::{apply_ui_patch, PatchMode};
use orkestr_protocol::{NewEvent, RpcError};

use super::{RpcContext, RpcRouter};
use crate::error::{events_to_rpc, storage_to_rpc};

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router.method::<m::PreviewPatch, _, _>(preview).method::<m::ApplyPatch, _, _>(apply);
}

fn rejected(issues: &[UiConfigIssue]) -> RpcError {
    let first = issues.first().map_or_else(String::new, |i| format!("{}: {}", i.path, i.message));
    RpcError::bad_request(format!("the UI change was rejected ({first})"))
        .with_data(serde_json::to_value(issues).unwrap_or(serde_json::Value::Null))
}

async fn preview(ctx: RpcContext, p: PreviewUiPatchParams) -> Result<PreviewUiPatchResult, RpcError> {
    let (base, version) = ctx.services.ui_config.current();
    let author = p.author.unwrap_or_default();
    let out = apply_ui_patch(&base, &p.patch, author, PatchMode::Strict).map_err(|i| rejected(&i))?;
    Ok(PreviewUiPatchResult {
        base,
        version,
        config: out.config,
        changes: out.changes,
        inverse: out.inverse,
        warnings: out.warnings,
    })
}

async fn apply(ctx: RpcContext, p: ApplyUiPatchParams) -> Result<ApplyUiPatchResult, RpcError> {
    if p.patch.is_empty() {
        return Err(RpcError::bad_request("the patch is empty"));
    }
    let services = &ctx.services;
    let store = &services.ui_config;
    let _guard = store.lock().await;
    let (base, version) = store.current();
    if let Some(expected) = p.base_version {
        if expected != version {
            return Err(RpcError::conflict(format!(
                "the UI configuration changed meanwhile (version {version}, expected {expected}); preview again"
            )));
        }
    }
    let author = p.author.unwrap_or_default();
    let out = apply_ui_patch(&base, &p.patch, author, PatchMode::Strict).map_err(|i| rejected(&i))?;
    if out.changes.is_empty() {
        return Ok(ApplyUiPatchResult {
            config: base,
            version,
            changes: Vec::new(),
            inverse: Vec::new(),
            warnings: out.warnings,
        });
    }
    let summary = p.summary.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or("UI change").to_owned();
    let new_version = store.commit(out.config.clone(), author, &summary).await.map_err(storage_to_rpc)?;
    tracing::info!(version = new_version, ?author, changes = out.changes.len(), "UI config patched");

    if base.performance_mode != out.config.performance_mode {
        let _g = services.settings.lock().await;
        let mut next = services.settings.get();
        if next.performance_mode != out.config.performance_mode {
            next.performance_mode = out.config.performance_mode;
            services.settings.save(next.clone(), &["performanceMode"]).await.map_err(storage_to_rpc)?;
            let payload = SettingsChangedPayload { settings: next, changed: vec!["performanceMode".into()] };
            services.events.publish(NewEvent::new(EventKind::SettingsChanged(payload))).await.map_err(events_to_rpc)?;
        }
    }

    let payload = UiConfigPatchedPayload {
        version: new_version,
        author,
        summary,
        config: out.config.clone(),
        proposal_id: p.proposal_id.clone(),
    };
    services.events.publish(NewEvent::new(EventKind::UiConfigPatched(payload))).await.map_err(events_to_rpc)?;
    Ok(ApplyUiPatchResult {
        config: out.config,
        version: new_version,
        changes: out.changes,
        inverse: out.inverse,
        warnings: out.warnings,
    })
}

