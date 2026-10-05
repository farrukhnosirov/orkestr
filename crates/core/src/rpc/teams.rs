//! `teams.*` handlers (§6, §68.27): Agent Teams CRUD and assignment of a
//! team to projects / Manager plans. Thin layer over
//! [`orkestr_orchestrator::Manager`] (logic in `orchestrator::teams`).

use orkestr_protocol::methods::teams::{
    self as t, AssignTeamParams, CreateTeamParams, ListTeamsParams, TeamIdParams, UpdateTeamParams,
};

use super::manager::manager_to_rpc;
use super::{RpcContext, RpcRouter};

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<t::List, _, _>(|ctx: RpcContext, p: ListTeamsParams| async move {
            ctx.services.manager.teams_list(p).await.map_err(manager_to_rpc)
        })
        .method::<t::Get, _, _>(|ctx: RpcContext, p: TeamIdParams| async move {
            ctx.services.manager.teams_get(p.team_id).await.map_err(manager_to_rpc)
        })
        .method::<t::Create, _, _>(|ctx: RpcContext, p: CreateTeamParams| async move {
            ctx.services.manager.teams_create(p).await.map_err(manager_to_rpc)
        })
        .method::<t::Update, _, _>(|ctx: RpcContext, p: UpdateTeamParams| async move {
            ctx.services.manager.teams_update(p).await.map_err(manager_to_rpc)
        })
        .method::<t::Delete, _, _>(|ctx: RpcContext, p: TeamIdParams| async move {
            ctx.services.manager.teams_delete(p.team_id).await.map_err(manager_to_rpc)
        })
        .method::<t::Assign, _, _>(|ctx: RpcContext, p: AssignTeamParams| async move {
            ctx.services.manager.teams_assign(p).await.map_err(manager_to_rpc)
        });
}
