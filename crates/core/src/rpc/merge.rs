//! `merge.*` handlers (§16, Phase 2). Thin layer over
//! [`orkestr_orchestrator::MergeManager`]. `merge.execute` returns at once
//! (awaiting approval, or merging under an `Auto` policy); progress, conflicts
//! and the result arrive as `MergeProposalUpdated` events.

use orkestr_protocol::methods::merge::{
    self as m, MergeAbortParams, MergeExecuteParams, MergeListParams, MergeListResult, MergePolicyParams,
    MergePolicyResult, MergeProposalIdParams, MergeProposalResult, MergeProposeParams, MergeReorderParams,
    MergeResolveConflictParams, MergeSetPolicyParams,
};

use super::manager::manager_to_rpc;
use super::{RpcContext, RpcRouter};

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::Propose, _, _>(|ctx: RpcContext, p: MergeProposeParams| async move {
            Ok(MergeProposalResult { proposal: ctx.services.merge.propose(p).await.map_err(manager_to_rpc)? })
        })
        .method::<m::Get, _, _>(|ctx: RpcContext, p: MergeProposalIdParams| async move {
            Ok(MergeProposalResult { proposal: ctx.services.merge.get(p.proposal_id).await.map_err(manager_to_rpc)? })
        })
        .method::<m::List, _, _>(|ctx: RpcContext, p: MergeListParams| async move {
            Ok(MergeListResult { proposals: ctx.services.merge.list(p).await.map_err(manager_to_rpc)? })
        })
        .method::<m::Reorder, _, _>(|ctx: RpcContext, p: MergeReorderParams| async move {
            Ok(MergeProposalResult {
                proposal: ctx.services.merge.reorder(p.proposal_id, p.order).await.map_err(manager_to_rpc)?,
            })
        })
        .method::<m::Execute, _, _>(|ctx: RpcContext, p: MergeExecuteParams| async move {
            Ok(MergeProposalResult { proposal: ctx.services.merge.execute(p).await.map_err(manager_to_rpc)? })
        })
        .method::<m::Abort, _, _>(|ctx: RpcContext, p: MergeAbortParams| async move {
            Ok(MergeProposalResult { proposal: ctx.services.merge.abort(p).await.map_err(manager_to_rpc)? })
        })
        .method::<m::ResolveConflict, _, _>(|ctx: RpcContext, p: MergeResolveConflictParams| async move {
            Ok(MergeProposalResult { proposal: ctx.services.merge.resolve_conflict(p).await.map_err(manager_to_rpc)? })
        })
        .method::<m::Policy, _, _>(|ctx: RpcContext, p: MergePolicyParams| async move {
            let policy = ctx.services.merge.policy(p.project_id).await.map_err(manager_to_rpc)?;
            Ok(MergePolicyResult { project_id: p.project_id, policy })
        })
        .method::<m::SetPolicy, _, _>(|ctx: RpcContext, p: MergeSetPolicyParams| async move {
            let policy = ctx.services.merge.set_policy(p.project_id, p.policy).await.map_err(manager_to_rpc)?;
            Ok(MergePolicyResult { project_id: p.project_id, policy })
        });
}
