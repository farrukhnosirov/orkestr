//! `secrets.*` handlers (§24). Values are write-only over RPC.

use orkestr_protocol::methods::emergency::LockVault;
use orkestr_protocol::methods::permissions::DeletedResult;
use orkestr_protocol::methods::secrets::{
    self as m, DeleteSecretParams, ListSecretsParams, ListSecretsResult, SecretBackendPreference, SecretResult,
    SetSecretParams, VaultStatusResult,
};
use orkestr_protocol::RpcError;
use orkestr_security::{SecretBackendChoice, VaultStatus};
use zeroize::Zeroizing;

use super::{RpcContext, RpcRouter};
use crate::error::security_to_rpc;

/// Registers the area (and `emergency.lockVault`, an alias of `secrets.lock`).
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::List, _, _>(|ctx: RpcContext, p: ListSecretsParams| async move {
            let secrets = ctx.services.security.secrets.list(p.project_id).await.map_err(security_to_rpc)?;
            Ok(ListSecretsResult { secrets })
        })
        .method::<m::Set, _, _>(set)
        .method::<m::Delete, _, _>(|ctx: RpcContext, p: DeleteSecretParams| async move {
            let sec = &ctx.services.security;
            let deleted = sec.secrets.delete(p.secret_id, sec.user()).await.map_err(security_to_rpc)?;
            Ok(DeletedResult { deleted })
        })
        .method::<m::Lock, _, _>(|ctx: RpcContext, _| async move { lock(&ctx).await })
        .method::<LockVault, _, _>(|ctx: RpcContext, _| async move { lock(&ctx).await })
        .method::<m::Unlock, _, _>(|ctx: RpcContext, _| async move {
            let sec = &ctx.services.security;
            Ok(view(sec.secrets.unlock(sec.user()).await.map_err(security_to_rpc)?))
        })
        .method::<m::Status, _, _>(|ctx: RpcContext, _| async move {
            Ok(view(ctx.services.security.secrets.status().await.map_err(security_to_rpc)?))
        });
}

/// Wire form of the vault status.
#[must_use]
pub fn view(s: VaultStatus) -> VaultStatusResult {
    VaultStatusResult {
        locked: s.locked,
        backend: match s.backend {
            SecretBackendChoice::Auto => SecretBackendPreference::Auto,
            SecretBackendChoice::Vault => SecretBackendPreference::Vault,
            SecretBackendChoice::VaultFileKey => SecretBackendPreference::VaultFileKey,
            SecretBackendChoice::Memory => SecretBackendPreference::Memory,
        },
        secret_count: s.secret_count,
    }
}

async fn lock(ctx: &RpcContext) -> Result<VaultStatusResult, RpcError> {
    let sec = &ctx.services.security;
    Ok(view(sec.secrets.lock(sec.user()).await.map_err(security_to_rpc)?))
}

async fn set(ctx: RpcContext, p: SetSecretParams) -> Result<SecretResult, RpcError> {
    let sec = &ctx.services.security;
    if let orkestr_domain::SecretScope::Project { project_id }
    | orkestr_domain::SecretScope::Environment { project_id, .. } = p.scope
    {
        super::permissions::load_project(&ctx, project_id).await?;
    }
    let SetSecretParams { name, scope, value } = p;
    let value = Zeroizing::new(value);
    let secret = sec.secrets.set(&name, scope, value, sec.user()).await.map_err(security_to_rpc)?;
    Ok(SecretResult { secret })
}
