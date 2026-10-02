//! `memory.*`, `decisions.*`, `knowledge.*` handlers (§20–21).
//!
//! Thin adapters over `orkestr-memory`; the engine validates input and
//! publishes `MemoryUpdated` / `DecisionRecorded`.

use orkestr_memory::MemoryError;
use orkestr_protocol::methods::memory::{
    self as m, DecisionListResult, DecisionResult, DeleteMemoryResult, EffectiveRulesResult, KnowledgeListResult,
    KnowledgeSourceResult, MemoryItemResult, MemoryListResult, RemoveKnowledgeResult, SearchKnowledgeResult,
    SearchMemoryResult,
};
use orkestr_protocol::RpcError;

use super::{RpcContext, RpcRouter};
use crate::error::storage_to_rpc;

/// Maps engine errors to RPC errors without leaking internals.
pub fn memory_to_rpc(err: MemoryError) -> RpcError {
    match err {
        MemoryError::NotFound(m) => RpcError::not_found(format!("{m} not found")),
        MemoryError::Invalid(m) => RpcError::bad_request(m),
        MemoryError::Conflict(m) => RpcError::conflict(m),
        MemoryError::Storage(e) => storage_to_rpc(e),
        MemoryError::Events(m) => {
            tracing::error!(error = %m, "memory event publication failed");
            RpcError::internal("change stored but event publication failed")
        }
    }
}

/// Registers the areas.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::List, _, _>(|ctx: RpcContext, p| async move {
            let items = ctx.services.memory.list_items(p).await.map_err(memory_to_rpc)?;
            Ok(MemoryListResult { items })
        })
        .method::<m::Get, _, _>(|ctx: RpcContext, p| async move {
            let item = ctx.services.memory.get_item(p.id).await.map_err(memory_to_rpc)?;
            Ok(MemoryItemResult { item })
        })
        .method::<m::Create, _, _>(|ctx: RpcContext, p| async move {
            let item = ctx.services.memory.create_item(p).await.map_err(memory_to_rpc)?;
            Ok(MemoryItemResult { item })
        })
        .method::<m::Update, _, _>(|ctx: RpcContext, p| async move {
            let item = ctx.services.memory.update_item(p.id, p.patch).await.map_err(memory_to_rpc)?;
            Ok(MemoryItemResult { item })
        })
        .method::<m::Delete, _, _>(|ctx: RpcContext, p| async move {
            let item = ctx.services.memory.delete_item(p.id).await.map_err(memory_to_rpc)?;
            Ok(DeleteMemoryResult { id: item.id })
        })
        .method::<m::Search, _, _>(|ctx: RpcContext, p| async move {
            let hits = ctx.services.memory.search(p).await.map_err(memory_to_rpc)?;
            Ok(SearchMemoryResult { hits })
        })
        .method::<m::EffectiveRules, _, _>(|ctx: RpcContext, p| async move {
            let rules = ctx.services.memory.effective_rules(p.project_id, p.kinds).await.map_err(memory_to_rpc)?;
            Ok(EffectiveRulesResult { rules })
        })
        .method::<m::DecisionsList, _, _>(|ctx: RpcContext, p| async move {
            let decisions =
                ctx.services.memory.list_decisions(p.project_id, p.status, p.limit).await.map_err(memory_to_rpc)?;
            Ok(DecisionListResult { decisions })
        })
        .method::<m::DecisionsGet, _, _>(|ctx: RpcContext, p| async move {
            let decision = ctx.services.memory.get_decision(p.id).await.map_err(memory_to_rpc)?;
            Ok(DecisionResult { decision })
        })
        .method::<m::DecisionsCreate, _, _>(|ctx: RpcContext, p| async move {
            let decision = ctx.services.memory.create_decision(p).await.map_err(memory_to_rpc)?;
            Ok(DecisionResult { decision })
        })
        .method::<m::DecisionsUpdate, _, _>(|ctx: RpcContext, p| async move {
            let decision = ctx.services.memory.update_decision(p.id, p.patch).await.map_err(memory_to_rpc)?;
            Ok(DecisionResult { decision })
        })
        .method::<m::KnowledgeList, _, _>(|ctx: RpcContext, p| async move {
            let sources = ctx
                .services
                .memory
                .list_knowledge(p.project_id, p.include_global.unwrap_or(false))
                .await
                .map_err(memory_to_rpc)?;
            Ok(KnowledgeListResult { sources })
        })
        .method::<m::KnowledgeAdd, _, _>(|ctx: RpcContext, p| async move {
            let source = ctx.services.memory.add_knowledge(p).await.map_err(memory_to_rpc)?;
            Ok(KnowledgeSourceResult { source })
        })
        .method::<m::KnowledgeRemove, _, _>(|ctx: RpcContext, p| async move {
            ctx.services.memory.remove_knowledge(p.id).await.map_err(memory_to_rpc)?;
            Ok(RemoveKnowledgeResult { id: p.id })
        })
        .method::<m::KnowledgeReindex, _, _>(|ctx: RpcContext, p| async move {
            let source = ctx.services.memory.reindex_knowledge(p.id).await.map_err(memory_to_rpc)?;
            Ok(KnowledgeSourceResult { source })
        })
        .method::<m::KnowledgeSearch, _, _>(|ctx: RpcContext, p| async move {
            let hits = ctx.services.memory.search_knowledge(p).await.map_err(memory_to_rpc)?;
            Ok(SearchKnowledgeResult { hits })
        });
}
