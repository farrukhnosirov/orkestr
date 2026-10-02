//! `agents.*` handlers (§6, §30, §57). Thin layer over
//! [`orkestr_agent_runtime::AgentRuntime`]; `agents.stop` is handled locally
//! with cancellation tokens (contract §6 — no provider round-trip).

use orkestr_agent_runtime::AgentError;
use orkestr_protocol::methods::agents::{
    self as m, AgentHandoffsResult, AgentIdParams, AgentStatsParams, AgentStopParams, ListAgentsParams,
    ListAgentsResult, SingleAgentResult, SpawnAgentParams, SpawnAgentResult, TranscriptParams,
};
use orkestr_protocol::{RpcError, RpcErrorCode};

use super::tasks::task_to_rpc;
use super::{RpcContext, RpcRouter};
use crate::error::storage_to_rpc;

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::List, _, _>(|ctx: RpcContext, p: ListAgentsParams| async move {
            Ok(ListAgentsResult { agents: ctx.services.agents.list(p).await.map_err(agent_to_rpc)? })
        })
        .method::<m::Get, _, _>(|ctx: RpcContext, p: AgentIdParams| async move {
            Ok(SingleAgentResult { agent: ctx.services.agents.get(p.agent_id).await.map_err(agent_to_rpc)? })
        })
        .method::<m::Spawn, _, _>(|ctx: RpcContext, p: SpawnAgentParams| async move {
            let (agent, task) = ctx.services.agents.spawn(p).await.map_err(agent_to_rpc)?;
            Ok(SpawnAgentResult { agent, task })
        })
        .method::<m::Stop, _, _>(|ctx: RpcContext, p: AgentStopParams| async move {
            let agent = ctx.services.agents.stop(p.agent_id, p.mode.unwrap_or_default()).await.map_err(agent_to_rpc)?;
            Ok(SingleAgentResult { agent })
        })
        .method::<m::Pause, _, _>(|ctx: RpcContext, p: AgentIdParams| async move {
            Ok(SingleAgentResult { agent: ctx.services.agents.pause(p.agent_id).await.map_err(agent_to_rpc)? })
        })
        .method::<m::Resume, _, _>(|ctx: RpcContext, p: AgentIdParams| async move {
            Ok(SingleAgentResult { agent: ctx.services.agents.resume(p.agent_id).await.map_err(agent_to_rpc)? })
        })
        .method::<m::Transcript, _, _>(|ctx: RpcContext, p: TranscriptParams| async move {
            ctx.services.agents.transcript(p.agent_id, p.limit).await.map_err(agent_to_rpc)
        })
        .method::<m::HandoffList, _, _>(|ctx: RpcContext, p: AgentIdParams| async move {
            Ok(AgentHandoffsResult { handoffs: ctx.services.agents.handoffs(p.agent_id).await.map_err(agent_to_rpc)? })
        })
        .method::<m::Stats, _, _>(|ctx: RpcContext, p: AgentStatsParams| async move {
            ctx.services.agents.stats(p.project_id).await.map_err(agent_to_rpc)
        });
}

/// Maps agent-runtime errors to RPC errors (contract §4 codes).
pub fn agent_to_rpc(err: AgentError) -> RpcError {
    match err {
        AgentError::NotFound(m) => RpcError::not_found(format!("{m} not found")),
        AgentError::Invalid(m) => RpcError::bad_request(m),
        AgentError::Conflict(m) => RpcError::conflict(m),
        AgentError::Forbidden(m) => RpcError::forbidden(m),
        AgentError::NoModel(m) => RpcError::new(RpcErrorCode::ProviderError, m),
        AgentError::Unimplemented(f) => RpcError::unimplemented_feature(&f),
        AgentError::Task(e) => task_to_rpc(e),
        AgentError::Storage(e) => storage_to_rpc(e),
        AgentError::Internal(m) => {
            tracing::error!(error = %m, "agent runtime failure while handling request");
            RpcError::internal("internal agent runtime error")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_mapping() {
        assert_eq!(agent_to_rpc(AgentError::NotFound("agent x".into())).code.as_str(), "not_found");
        assert_eq!(agent_to_rpc(AgentError::Forbidden("x".into())).code.as_str(), "forbidden");
        assert_eq!(agent_to_rpc(AgentError::NoModel("x".into())).code.as_str(), "provider_error");
        assert_eq!(agent_to_rpc(AgentError::Conflict("x".into())).code.as_str(), "conflict");
        let e = agent_to_rpc(AgentError::Internal("secret path".into()));
        assert!(!e.message.contains("secret"));
    }
}
