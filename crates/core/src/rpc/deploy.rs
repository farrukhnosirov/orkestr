//! Environments + deployments (`environments.*`, `deploy.*`, ADR-0041). **Owner: WP4.**
//!
//! Thin handlers over [`orkestr_deploy::DeployService`]: they resolve the
//! project (root path, remote project scope) and the calling actor. Deploys to
//! protected environments come back `AwaitingApproval` with the approval id
//! and run after the approval is granted.

use std::path::PathBuf;

use orkestr_domain::{Actor, EnvironmentId, ProjectId};
use orkestr_protocol::methods::deploy as m;
use orkestr_protocol::methods::Empty;
use orkestr_protocol::RpcError;

use super::permissions::load_project;
use super::{Principal, RpcContext, RpcRouter};

fn actor(ctx: &RpcContext) -> Actor {
    match &ctx.principal {
        Principal::Local => ctx.services.security.user(),
        Principal::Remote(rp) => Actor::RemoteDevice(rp.device_id),
    }
}

/// Remote callers restricted to some projects may only touch those.
fn check_scope(ctx: &RpcContext, project: ProjectId) -> Result<(), RpcError> {
    if let Principal::Remote(rp) = &ctx.principal {
        if !rp.project_ids.is_empty() && !rp.project_ids.contains(&project) {
            return Err(RpcError::forbidden("this device has no access to the project"));
        }
    }
    Ok(())
}

async fn project_root(ctx: &RpcContext, project: ProjectId) -> Result<PathBuf, RpcError> {
    check_scope(ctx, project)?;
    let p = load_project(ctx, project).await?;
    p.root_path
        .map(PathBuf::from)
        .ok_or_else(|| RpcError::bad_request("the project has no local folder to deploy from"))
}

async fn env_project(ctx: &RpcContext, env: EnvironmentId) -> Result<ProjectId, RpcError> {
    let rec = ctx.services.deploy.environment(env).await?;
    check_scope(ctx, rec.environment.project_id)?;
    Ok(rec.environment.project_id)
}

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::EnvironmentsList, _, _>(|ctx: RpcContext, p: m::ListEnvironmentsParams| async move {
            check_scope(&ctx, p.project_id)?;
            load_project(&ctx, p.project_id).await?;
            Ok(m::ListEnvironmentsResult { environments: ctx.services.deploy.list_environments(p.project_id).await? })
        })
        .method::<m::EnvironmentsCreate, _, _>(|ctx: RpcContext, p: m::CreateEnvironmentParams| async move {
            check_scope(&ctx, p.project_id)?;
            load_project(&ctx, p.project_id).await?;
            let by = actor(&ctx);
            Ok(m::EnvironmentResult { environment: ctx.services.deploy.create_environment(p, by).await? })
        })
        .method::<m::EnvironmentsUpdate, _, _>(|ctx: RpcContext, p: m::UpdateEnvironmentParams| async move {
            env_project(&ctx, p.environment_id).await?;
            let by = actor(&ctx);
            Ok(m::EnvironmentResult { environment: ctx.services.deploy.update_environment(p, by).await? })
        })
        .method::<m::EnvironmentsDelete, _, _>(|ctx: RpcContext, p: m::EnvironmentIdParams| async move {
            env_project(&ctx, p.environment_id).await?;
            ctx.services.deploy.delete_environment(p.environment_id, actor(&ctx)).await?;
            Ok(Empty {})
        })
        .method::<m::Adapters, _, _>(|ctx: RpcContext, _p: Empty| async move {
            Ok(m::AdaptersResult { adapters: ctx.services.deploy.adapter_infos().await? })
        })
        .method::<m::Plan, _, _>(|ctx: RpcContext, p: m::DeployParams| async move {
            let root = project_root(&ctx, env_project(&ctx, p.environment_id).await?).await?;
            Ok(m::DeployPlanResult { plan: ctx.services.deploy.plan(&p, &root).await? })
        })
        .method::<m::Start, _, _>(|ctx: RpcContext, p: m::DeployParams| async move {
            let root = project_root(&ctx, env_project(&ctx, p.environment_id).await?).await?;
            Ok(ctx.services.deploy.start(&p, &root, actor(&ctx)).await?)
        })
        .method::<m::Cancel, _, _>(|ctx: RpcContext, p: m::DeploymentIdParams| async move {
            let d = ctx.services.deploy.get(p.deployment_id).await?;
            check_scope(&ctx, d.deployment.project_id)?;
            Ok(ctx.services.deploy.cancel(p.deployment_id, actor(&ctx)).await?)
        })
        .method::<m::List, _, _>(|ctx: RpcContext, p: m::ListDeploymentsParams| async move {
            check_scope(&ctx, p.project_id)?;
            Ok(m::ListDeploymentsResult {
                deployments: ctx.services.deploy.list(p.project_id, p.environment_id, p.limit).await?,
            })
        })
        .method::<m::Get, _, _>(|ctx: RpcContext, p: m::DeploymentIdParams| async move {
            let d = ctx.services.deploy.get(p.deployment_id).await?;
            check_scope(&ctx, d.deployment.project_id)?;
            Ok(d)
        })
        .method::<m::Logs, _, _>(|ctx: RpcContext, p: m::DeployLogsParams| async move {
            let d = ctx.services.deploy.get(p.deployment_id).await?;
            check_scope(&ctx, d.deployment.project_id)?;
            Ok(ctx.services.deploy.logs(p.deployment_id, p.offset.unwrap_or(0)).await?)
        })
        .method::<m::Rollback, _, _>(|ctx: RpcContext, p: m::RollbackDeploymentParams| async move {
            let root = project_root(&ctx, env_project(&ctx, p.environment_id).await?).await?;
            Ok(ctx.services.deploy.rollback(&p, &root, actor(&ctx)).await?)
        });
}
