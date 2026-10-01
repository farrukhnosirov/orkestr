//! `events.*` handlers (Replay §21, reconnect gap filling).

use orkestr_events::ReplayQuery;
use orkestr_protocol::methods::events::{Replay, ReplayParams, ReplayResult, MAX_REPLAY_LIMIT};
use orkestr_protocol::RpcError;

use super::{RpcContext, RpcRouter};
use crate::error::events_to_rpc;

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router.method::<Replay, _, _>(replay);
}

async fn replay(ctx: RpcContext, p: ReplayParams) -> Result<ReplayResult, RpcError> {
    let from_seq = p.from_seq.unwrap_or(1).max(1);
    if let Some(to) = p.to_seq {
        if to < from_seq {
            return Err(RpcError::bad_request("toSeq must be >= fromSeq"));
        }
    }
    let query = ReplayQuery {
        from_seq,
        to_seq: p.to_seq,
        limit: p.limit.unwrap_or(MAX_REPLAY_LIMIT).clamp(1, MAX_REPLAY_LIMIT),
        topics: p.topics,
        newest_first: p.newest_first.unwrap_or(false),
    };
    let page = ctx.services.events.replay(&query).await.map_err(events_to_rpc)?;
    Ok(ReplayResult { events: page.events, last_seq: ctx.services.events.last_seq(), has_more: page.has_more })
}
