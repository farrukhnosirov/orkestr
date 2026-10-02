//! `approvals.*` handlers (§23).

use orkestr_protocol::methods::approvals::{
    self as m, ApprovalDecisionKind, ApprovalIdParams, ApprovalResult, ListApprovalsParams, ListApprovalsResult, ResolveApprovalParams,
};
use orkestr_protocol::RpcError;
use orkestr_security::{ApprovalDecision, ApprovalFilter};

use super::{RpcContext, RpcRouter};
use crate::error::security_to_rpc;

/// Max note length.
const MAX_NOTE: usize = 2000;

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::List, _, _>(|ctx: RpcContext, p: ListApprovalsParams| async move {
            let approvals = ctx
                .services
                .security
                .approvals
                .list(&ApprovalFilter { status: p.status, project_id: p.project_id, limit: p.limit })
                .await
                .map_err(security_to_rpc)?;
            Ok(ListApprovalsResult { approvals })
        })
        .method::<m::Get, _, _>(|ctx: RpcContext, p: ApprovalIdParams| async move {
            let approval = ctx
                .services
                .security
                .approvals
                .get(p.approval_id)
                .await
                .map_err(security_to_rpc)?
                .ok_or_else(|| RpcError::not_found(format!("approval {} not found", p.approval_id)))?;
            Ok(ApprovalResult { approval })
        })
        .method::<m::Resolve, _, _>(|ctx: RpcContext, p: ResolveApprovalParams| async move {
            if p.note.as_ref().is_some_and(|n| n.chars().count() > MAX_NOTE) {
                return Err(RpcError::bad_request(format!("note must be at most {MAX_NOTE} characters")));
            }
            let sec = &ctx.services.security;
            let decision = match p.decision {
                ApprovalDecisionKind::Approve => ApprovalDecision::Approve,
                ApprovalDecisionKind::Reject => ApprovalDecision::Reject,
            };
            let approval = sec.approvals.resolve(p.approval_id, decision, p.note, sec.user()).await.map_err(security_to_rpc)?;
            Ok(ApprovalResult { approval })
        });
}
