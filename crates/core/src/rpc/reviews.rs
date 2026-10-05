//! `reviews.*` handlers (§8, §17, §68.12): automatic code review and
//! adversarial review of a task. Thin layer over
//! [`orkestr_orchestrator::Manager`]; progress streams as `ReviewUpdated` /
//! `ReviewFinished` events.

use orkestr_protocol::methods::reviews::{
    self as m, AdversarialReviewParams, GetReviewParams, ListReviewsParams, RequestReviewParams, ResolveFindingParams,
};

use super::manager::manager_to_rpc;
use super::{RpcContext, RpcRouter};

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::Request, _, _>(|ctx: RpcContext, p: RequestReviewParams| async move {
            ctx.services.manager.request_review(p).await.map_err(manager_to_rpc)
        })
        .method::<m::List, _, _>(|ctx: RpcContext, p: ListReviewsParams| async move {
            ctx.services.manager.list_reviews(p).await.map_err(manager_to_rpc)
        })
        .method::<m::Get, _, _>(|ctx: RpcContext, p: GetReviewParams| async move {
            ctx.services.manager.get_review(p.review_id).await.map_err(manager_to_rpc)
        })
        .method::<m::ResolveFinding, _, _>(|ctx: RpcContext, p: ResolveFindingParams| async move {
            ctx.services.manager.resolve_finding(p).await.map_err(manager_to_rpc)
        })
        .method::<m::Adversarial, _, _>(|ctx: RpcContext, p: AdversarialReviewParams| async move {
            ctx.services.manager.adversarial_review(p).await.map_err(manager_to_rpc)
        });
}
