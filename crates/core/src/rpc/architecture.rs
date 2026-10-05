//! `architecture.*` handlers (§21, §68.48). Thin layer over
//! [`orkestr_insight::ArchitectureService`]: the project root is resolved
//! here, scanning / snapshots / diffs live in the insight crate.

use orkestr_insight::InsightError;
use orkestr_protocol::methods::architecture::{
    self as a, ArchDiffParams, ArchDiffResult, ArchMapParams, ArchMapResult, ArchRefreshParams, ArchRefreshResult,
};
use orkestr_protocol::RpcError;

use super::vcs::project_root;
use super::{RpcContext, RpcRouter};
use crate::error::storage_to_rpc;

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<a::Map, _, _>(|ctx: RpcContext, p: ArchMapParams| async move {
            let svc = &ctx.services.architecture;
            let map = match svc.get(p.project_id, p.snapshot_id.as_deref()).await.map_err(insight_to_rpc)? {
                Some(map) => map,
                None => {
                    // First open: scan once (later refreshes are incremental / automatic).
                    let root = project_root(&ctx, p.project_id).await?;
                    svc.refresh(p.project_id, root, false, false).await.map_err(insight_to_rpc)?.map
                }
            };
            let snapshots = svc.snapshots(p.project_id).await.map_err(insight_to_rpc)?;
            Ok(ArchMapResult { map, snapshots })
        })
        .method::<a::Refresh, _, _>(|ctx: RpcContext, p: ArchRefreshParams| async move {
            let root = project_root(&ctx, p.project_id).await?;
            let out = ctx
                .services
                .architecture
                .refresh(p.project_id, root, p.full.unwrap_or(false), false)
                .await
                .map_err(insight_to_rpc)?;
            Ok(ArchRefreshResult { map: out.map, changed: out.changed, diff: out.diff })
        })
        .method::<a::Diff, _, _>(|ctx: RpcContext, p: ArchDiffParams| async move {
            let diff = ctx
                .services
                .architecture
                .diff(p.project_id, p.from.as_deref(), p.to.as_deref())
                .await
                .map_err(insight_to_rpc)?;
            Ok(ArchDiffResult { diff })
        });
}

/// Maps insight errors to RPC errors without leaking internals.
pub fn insight_to_rpc(err: InsightError) -> RpcError {
    match err {
        InsightError::NotFound(m) => RpcError::not_found(format!("{m} not found")),
        InsightError::Invalid(m) => RpcError::bad_request(m),
        InsightError::Storage(e) => storage_to_rpc(e),
        other => {
            tracing::error!(error = %other, "insight failure while handling request");
            RpcError::internal("internal insight error")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_mapping() {
        assert_eq!(insight_to_rpc(InsightError::NotFound("snapshot x".into())).code.as_str(), "not_found");
        assert_eq!(insight_to_rpc(InsightError::Invalid("x".into())).code.as_str(), "bad_request");
        let e = insight_to_rpc(InsightError::Aborted("C:/secret/path".into()));
        assert_eq!(e.code.as_str(), "internal");
        assert!(!e.message.contains("secret"));
    }
}
