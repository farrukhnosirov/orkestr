//! `audit.*` handlers.

use orkestr_protocol::methods::audit::{self as m, AuditEntry, ListAuditParams, ListAuditResult};
use orkestr_security::AuditQuery;

use super::{RpcContext, RpcRouter};
use crate::error::security_to_rpc;

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router.method::<m::List, _, _>(|ctx: RpcContext, p: ListAuditParams| async move {
        let audit = &ctx.services.security.audit;
        let rows = audit
            .query(AuditQuery { project_id: p.project_id, action_prefix: p.action, before_seq: p.before_seq, limit: p.limit })
            .await
            .map_err(security_to_rpc)?;
        let chain_valid = if p.verify.unwrap_or(false) {
            Some(audit.verify_chain().await.map_err(security_to_rpc)?.is_none())
        } else {
            None
        };
        Ok(ListAuditResult { entries: rows.into_iter().map(|r| AuditEntry { seq: r.seq, event: r.event }).collect(), chain_valid })
    });
}
