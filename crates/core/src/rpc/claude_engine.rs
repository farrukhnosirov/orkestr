//! `claudeEngine.*` handlers (logic in [`crate::claude_engine`]).

use orkestr_protocol::methods::claude_engine::{
    self as m, ClaudeAgentsParams, ClaudeEngineStatusParams, DeleteClaudeAgentParams, SaveClaudeAgentParams,
    UseClaudeForEverythingParams,
};
use orkestr_protocol::RpcError;

use super::{RpcContext, RpcRouter};
use crate::claude_engine as engine;

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::Status, _, _>(|ctx: RpcContext, p: ClaudeEngineStatusParams| async move {
            Ok::<_, RpcError>(engine::status(&ctx.services, p.refresh).await)
        })
        .method::<m::UseForEverything, _, _>(|ctx: RpcContext, p: UseClaudeForEverythingParams| async move {
            engine::use_for_everything(&ctx.services, p.model).await
        })
        .method::<m::Agents, _, _>(|ctx: RpcContext, p: ClaudeAgentsParams| async move {
            engine::agents(&ctx.services, p.project_id, p.refresh).await
        })
        .method::<m::SaveAgent, _, _>(|ctx: RpcContext, p: SaveClaudeAgentParams| async move {
            engine::save_agent(&ctx.services, p).await
        })
        .method::<m::DeleteAgent, _, _>(|ctx: RpcContext, p: DeleteClaudeAgentParams| async move {
            engine::delete_agent(&ctx.services, p).await
        });
}
