//! `templates.*` handlers (§6). Thin layer over
//! [`orkestr_agent_runtime::TemplateService`].

use orkestr_protocol::methods::templates::{
    self as m, CreateTemplateParams, DeleteTemplateResult, ExportTemplateResult, ImportTemplateParams,
    ImportTemplateResult, ListTemplatesParams, ListTemplatesResult, SingleTemplateResult, TemplateIdParams,
    UpdateTemplateParams,
};

use super::agents::agent_to_rpc;
use super::{RpcContext, RpcRouter};

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::List, _, _>(|ctx: RpcContext, p: ListTemplatesParams| async move {
            let templates = ctx.services.agents.templates().list(p.role, p.source).await.map_err(agent_to_rpc)?;
            Ok(ListTemplatesResult { templates })
        })
        .method::<m::Get, _, _>(|ctx: RpcContext, p: TemplateIdParams| async move {
            let template = ctx.services.agents.templates().get(p.template_id).await.map_err(agent_to_rpc)?;
            Ok(SingleTemplateResult { template })
        })
        .method::<m::Create, _, _>(|ctx: RpcContext, p: CreateTemplateParams| async move {
            let template = ctx.services.agents.templates().create(p.template).await.map_err(agent_to_rpc)?;
            Ok(SingleTemplateResult { template })
        })
        .method::<m::Update, _, _>(|ctx: RpcContext, p: UpdateTemplateParams| async move {
            let template =
                ctx.services.agents.templates().update(p.template_id, p.template).await.map_err(agent_to_rpc)?;
            Ok(SingleTemplateResult { template })
        })
        .method::<m::Delete, _, _>(|ctx: RpcContext, p: TemplateIdParams| async move {
            let template_id = ctx.services.agents.templates().delete(p.template_id).await.map_err(agent_to_rpc)?;
            Ok(DeleteTemplateResult { template_id })
        })
        .method::<m::Export, _, _>(|ctx: RpcContext, p: TemplateIdParams| async move {
            let (json, file_name) =
                ctx.services.agents.templates().export(p.template_id).await.map_err(agent_to_rpc)?;
            Ok(ExportTemplateResult { json, file_name })
        })
        .method::<m::Import, _, _>(|ctx: RpcContext, p: ImportTemplateParams| async move {
            let (template, dropped_permissions) = ctx
                .services
                .agents
                .templates()
                .import(&p.json, p.accept_permissions.unwrap_or(false))
                .await
                .map_err(agent_to_rpc)?;
            Ok(ImportTemplateResult { template, dropped_permissions })
        });
}
