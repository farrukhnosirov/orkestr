//! `sync.*` handlers — encrypted sync + Seamless Continue (Phase 4, ADR-0031).
//!
//! `sync.rotateKeys` stays `unimplementedFeature` (RR-G14, deferred).

use std::path::PathBuf;
use std::sync::Arc;

use orkestr_domain::{Project, ProjectId};
use orkestr_protocol::methods::sync as m;
use orkestr_protocol::methods::tasks::ListTasksParams;
use orkestr_protocol::methods::Empty;
use orkestr_protocol::{RpcError, RpcErrorCode};
use orkestr_sync::{RemoteSyncConfig, Resolution, SyncEngine, SyncError, SyncReport, SyncStatus};
use serde_json::json;

use super::{stub, RpcContext, RpcRouter};

/// Record keys of the `records:` stream used by Seamless Continue.
pub const UI_STATE_KEY: &str = "uiState";
/// Task summary record.
pub const TASK_SUMMARY_KEY: &str = "taskSummary";

/// Maps engine errors onto RPC errors (`data.syncError` names the kind).
#[must_use]
pub fn sync_to_rpc(e: SyncError) -> RpcError {
    let (code, kind) = match &e {
        SyncError::NotAllowed | SyncError::PolicyDenied(_) => (RpcErrorCode::Forbidden, "policyDenied"),
        SyncError::NotConfigured => (RpcErrorCode::Conflict, "notConfigured"),
        SyncError::Config(_) => (RpcErrorCode::BadRequest, "config"),
        SyncError::NotFound => (RpcErrorCode::NotFound, "notFound"),
        SyncError::Integrity(_) => (RpcErrorCode::Conflict, "integrity"),
        SyncError::Network(_) => (RpcErrorCode::Conflict, "network"),
        SyncError::Backend(_) | SyncError::Storage(_) => (RpcErrorCode::Internal, "storage"),
    };
    RpcError::new(code, e.to_string()).with_data(json!({ "syncError": kind }))
}

fn engine(ctx: &RpcContext) -> Result<Arc<dyn SyncEngine>, RpcError> {
    ctx.services.sync.clone().ok_or_else(|| RpcError::unimplemented_feature("sync"))
}

async fn project(ctx: &RpcContext, id: ProjectId) -> Result<Project, RpcError> {
    ctx.services
        .projects
        .get(id)
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?
        .ok_or_else(|| RpcError::not_found(format!("project {id} not found")))
}

/// Loads the project and enforces its privacy policy (`No Remote Sync` ⇒ forbidden).
async fn allowed_project(ctx: &RpcContext, id: ProjectId) -> Result<Project, RpcError> {
    let p = project(ctx, id).await?;
    orkestr_sync::check_policy(&p.effective_privacy()).map_err(sync_to_rpc)?;
    Ok(p)
}

fn status_info(project_id: ProjectId, s: SyncStatus) -> m::SyncStatusInfo {
    m::SyncStatusInfo {
        project_id,
        configured: s.configured,
        last_sync_at: s.last_sync_at,
        pending: s.pending,
        conflicts: s.conflicts,
        last_error: s.last_error,
        generated_remote_key: None,
    }
}

fn now_result(r: SyncReport) -> m::SyncNowResult {
    m::SyncNowResult { pushed: r.pushed, pulled: r.pulled, bytes: r.bytes, conflicts: r.conflicts }
}

/// Gateway URL + core token for `/v1/sync` when remote access is enabled.
///
fn gateway_credentials(ctx: &RpcContext) -> Option<(String, String)> {
    ctx.services.remote.sync_credentials()
}

async fn configure(ctx: RpcContext, p: m::SyncConfigureParams) -> Result<m::SyncStatusInfo, RpcError> {
    if !ctx.principal.is_local() {
        return Err(RpcError::forbidden("sync.configure is local-only"));
    }
    let eng = engine(&ctx)?;
    if p.disable == Some(true) {
        eng.disable(p.project_id).await.map_err(sync_to_rpc)?;
        return Ok(status_info(p.project_id, eng.status(p.project_id).await.map_err(sync_to_rpc)?));
    }
    let proj = allowed_project(&ctx, p.project_id).await?;
    let root = proj
        .root_path
        .clone()
        .ok_or_else(|| RpcError::bad_request("the project is not bound to a folder; sync needs a root path"))?;
    // Explicit server URL + token win; otherwise the paired gateway's
    // credentials (remote access, ADR-0030) are used.
    let gateway = gateway_credentials(&ctx);
    let server_url = p
        .server_url
        .filter(|u| !u.trim().is_empty())
        .or_else(|| gateway.as_ref().map(|g| g.0.clone()))
        .ok_or_else(|| RpcError::bad_request("serverUrl is required (remote access is not enabled)"))?;
    let token = p
        .access_token
        .filter(|t| !t.is_empty())
        .or_else(|| gateway.as_ref().map(|g| g.1.clone()))
        .ok_or_else(|| RpcError::bad_request("accessToken is required"))?;
    let (remote_key, generated) = match p.remote_key.filter(|k| !k.trim().is_empty()) {
        Some(k) => (k.trim().to_owned(), None),
        None => {
            let k = orkestr_sync::crypto::generate_remote_key();
            (k.clone(), Some(k))
        }
    };
    let project_key = p.remote_project.filter(|k| !k.trim().is_empty()).unwrap_or_else(|| proj.slug.clone());
    crate::redact::register_secret(&token);
    crate::redact::register_secret(&remote_key);
    eng.configure(
        p.project_id,
        PathBuf::from(root),
        RemoteSyncConfig {
            server_url: server_url.trim().to_owned(),
            access_token: token,
            remote_key,
            project_key: Some(project_key),
        },
    )
    .await
    .map_err(sync_to_rpc)?;
    let mut info = status_info(p.project_id, eng.status(p.project_id).await.map_err(sync_to_rpc)?);
    info.generated_remote_key = generated;
    Ok(info)
}

/// Builds the task summary record (task list + recent activity counts).
async fn task_summary(ctx: &RpcContext, project_id: ProjectId) -> Result<serde_json::Value, RpcError> {
    let tasks = ctx
        .services
        .tasks
        .list(ListTasksParams { project_id: Some(project_id), ..ListTasksParams::default() })
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?;
    let items: Vec<serde_json::Value> =
        tasks.iter().map(|t| json!({ "id": t.id, "title": t.title, "status": t.status })).collect();
    let since = ctx.services.previous_session_started_at;
    let activity = crate::resume::summarize(&ctx.services.db, since, Some(project_id), since, false)
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?;
    let mut text = if tasks.is_empty() {
        "No tasks.".to_owned()
    } else {
        let list: Vec<String> = tasks.iter().take(10).map(|t| format!("{} — {:?}", t.title, t.status)).collect();
        format!("{} task(s): {}", tasks.len(), list.join("; "))
    };
    if activity.total_events > 0 {
        let top: Vec<String> = activity.counts.iter().take(3).map(|c| format!("{} {}", c.count, c.kind)).collect();
        text.push_str(&format!(". Recent activity: {}", top.join(", ")));
    }
    Ok(json!({ "text": text, "tasks": items, "totalEvents": activity.total_events }))
}

async fn continue_on(ctx: RpcContext, p: m::SyncContinueParams) -> Result<m::SyncContinueResult, RpcError> {
    let eng = engine(&ctx)?;
    allowed_project(&ctx, p.project_id).await?;
    let ui = p.ui_state.unwrap_or_else(|| json!({ "projectId": p.project_id }));
    eng.put_record(p.project_id, UI_STATE_KEY, ui).await.map_err(sync_to_rpc)?;
    let summary = task_summary(&ctx, p.project_id).await?;
    let text = summary.get("text").and_then(|t| t.as_str()).map(str::to_owned);
    eng.put_record(p.project_id, TASK_SUMMARY_KEY, summary).await.map_err(sync_to_rpc)?;
    let report = eng.sync_now(p.project_id).await.map_err(sync_to_rpc)?;
    Ok(m::SyncContinueResult { published_at: orkestr_domain::now(), report: now_result(report), summary: text })
}

async fn resume(ctx: RpcContext, p: m::SyncProjectParams) -> Result<m::SyncResumeResult, RpcError> {
    let eng = engine(&ctx)?;
    allowed_project(&ctx, p.project_id).await?;
    let report = eng.sync_now(p.project_id).await.map_err(sync_to_rpc)?;
    let ui_state = eng.get_record(p.project_id, UI_STATE_KEY).await.map_err(sync_to_rpc)?;
    let summary = eng
        .get_record(p.project_id, TASK_SUMMARY_KEY)
        .await
        .map_err(sync_to_rpc)?
        .and_then(|s| s.get("text").and_then(|t| t.as_str()).map(str::to_owned));
    Ok(m::SyncResumeResult { report: now_result(report), ui_state, summary })
}

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router.method::<m::Configure, _, _>(configure);
    router.method::<m::Status, _, _>(|ctx: RpcContext, p: m::SyncProjectParams| async move {
        let eng = engine(&ctx)?;
        Ok(status_info(p.project_id, eng.status(p.project_id).await.map_err(sync_to_rpc)?))
    });
    router.method::<m::Now, _, _>(|ctx: RpcContext, p: m::SyncProjectParams| async move {
        let eng = engine(&ctx)?;
        allowed_project(&ctx, p.project_id).await?;
        Ok(now_result(eng.sync_now(p.project_id).await.map_err(sync_to_rpc)?))
    });
    router.method::<m::ContinueOn, _, _>(continue_on);
    router.method::<m::Resume, _, _>(resume);
    router.method::<m::ListConflicts, _, _>(|ctx: RpcContext, p: m::SyncProjectParams| async move {
        let eng = engine(&ctx)?;
        let conflicts = eng
            .list_conflicts(p.project_id)
            .await
            .map_err(sync_to_rpc)?
            .into_iter()
            .map(|c| m::SyncConflictInfo {
                id: c.id.0,
                project_id: c.project_id,
                path: c.path,
                conflict_copy: c.conflict_copy,
                detected_at: c.detected_at,
            })
            .collect();
        Ok(m::SyncConflictList { conflicts })
    });
    router.method::<m::ResolveConflict, _, _>(|ctx: RpcContext, p: m::SyncResolveConflictParams| async move {
        let eng = engine(&ctx)?;
        let ours = eng.list_conflicts(p.project_id).await.map_err(sync_to_rpc)?;
        if !ours.iter().any(|c| c.id.0 == p.conflict_id) {
            return Err(RpcError::not_found(format!("conflict {} not found", p.conflict_id)));
        }
        let r = match p.resolution {
            m::SyncResolution::KeepLocal => Resolution::KeepLocal,
            m::SyncResolution::KeepRemote => Resolution::KeepRemote,
            m::SyncResolution::KeepBoth => Resolution::KeepBoth,
        };
        eng.resolve_conflict(orkestr_sync::ConflictId(p.conflict_id), r).await.map_err(sync_to_rpc)?;
        Ok(Empty::default())
    });
    stub::<m::RotateKeys>(router);
}
