//! `consensus.*` handlers (§8, §68.13): independent proposals from 2–5
//! models + a synthesized decision recorded in the Decision Log. Thin layer
//! over [`orkestr_orchestrator::Manager`]; progress streams as
//! `ConsensusUpdated` events.

use orkestr_protocol::methods::consensus::{self as m, GetConsensusParams, ListConsensusParams, RunConsensusParams};

use super::manager::manager_to_rpc;
use super::{RpcContext, RpcRouter};

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::Run, _, _>(|ctx: RpcContext, p: RunConsensusParams| async move {
            ctx.services.manager.run_consensus(p).await.map_err(manager_to_rpc)
        })
        .method::<m::List, _, _>(|ctx: RpcContext, p: ListConsensusParams| async move {
            ctx.services.manager.list_consensus(p).await.map_err(manager_to_rpc)
        })
        .method::<m::Get, _, _>(|ctx: RpcContext, p: GetConsensusParams| async move {
            ctx.services.manager.get_consensus(p.consensus_id).await.map_err(manager_to_rpc)
        });
}
