//! `pm.*` handlers (§9, §68.47): the AI Product Manager — idea → epic →
//! features → stories → acceptance criteria → tasks, as an editable draft
//! that becomes task-engine tasks on approval. Logic in `orchestrator::pm`.

use orkestr_protocol::methods::pm::{
    self as m, PmApproveParams, PmDraftIdParams, PmDraftParams, PmListParams, PmUpdateParams,
};

use super::manager::manager_to_rpc;
use super::{RpcContext, RpcRouter};

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::Draft, _, _>(|ctx: RpcContext, p: PmDraftParams| async move {
            ctx.services.manager.pm_draft(p).await.map_err(manager_to_rpc)
        })
        .method::<m::Get, _, _>(|ctx: RpcContext, p: PmDraftIdParams| async move {
            ctx.services.manager.pm_get(&p.draft_id).await.map_err(manager_to_rpc)
        })
        .method::<m::List, _, _>(|ctx: RpcContext, p: PmListParams| async move {
            ctx.services.manager.pm_list(p).await.map_err(manager_to_rpc)
        })
        .method::<m::Update, _, _>(|ctx: RpcContext, p: PmUpdateParams| async move {
            ctx.services.manager.pm_update(p).await.map_err(manager_to_rpc)
        })
        .method::<m::Approve, _, _>(|ctx: RpcContext, p: PmApproveParams| async move {
            ctx.services.manager.pm_approve(p).await.map_err(manager_to_rpc)
        })
        .method::<m::Discard, _, _>(|ctx: RpcContext, p: PmDraftIdParams| async move {
            ctx.services.manager.pm_discard(&p.draft_id).await.map_err(manager_to_rpc)
        });
}
