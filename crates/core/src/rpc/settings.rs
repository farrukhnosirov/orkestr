//! `settings.*` handlers.
//!
//! `settings.update` validates the patch, performs side effects **before**
//! persisting (autostart registration — if it fails nothing is saved), writes
//! the changed keys and publishes `SettingsChanged` on `global`.

use std::sync::Arc;

use orkestr_platform::{AutostartEntry, PlatformError};
use orkestr_protocol::event::{EventKind, SettingsChangedPayload};
use orkestr_protocol::methods::settings::{self as m, AppSettings, SettingsResult, UpdateSettingsParams};
use orkestr_protocol::{NewEvent, RpcError};

use super::{RpcContext, RpcRouter};
use crate::error::{events_to_rpc, storage_to_rpc};
use crate::services::Services;

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router.method::<m::Get, _, _>(|ctx: RpcContext, _| get(ctx)).method::<m::Update, _, _>(update);
}

fn platform_to_rpc(context: &str, e: PlatformError) -> RpcError {
    match e {
        PlatformError::NotSupported { .. } | PlatformError::Invalid(_) => {
            RpcError::bad_request(format!("{context}: {e}"))
        }
        other => {
            tracing::warn!(error = %other, "{context}");
            RpcError::internal(format!("{context}: {other}"))
        }
    }
}

fn autostart_supported() -> bool {
    cfg!(any(target_os = "windows", target_os = "macos", target_os = "linux"))
}

async fn autostart_registered(services: &Arc<Services>) -> Option<bool> {
    let s = Arc::clone(services);
    tokio::task::spawn_blocking(move || s.autostart_backend().is_enabled(&s.autostart.id).ok()).await.ok().flatten()
}

async fn set_autostart(services: &Arc<Services>, enabled: bool) -> Result<(), RpcError> {
    let s = Arc::clone(services);
    tokio::task::spawn_blocking(move || -> Result<(), PlatformError> {
        let backend = s.autostart_backend();
        if enabled {
            let entry = match &s.autostart.program {
                Some(p) => {
                    AutostartEntry { id: s.autostart.id.clone(), program: p.clone(), args: s.autostart.args.clone() }
                }
                None => AutostartEntry::for_current_exe(s.autostart.id.clone(), s.autostart.args.clone())?,
            };
            backend.enable(&entry)
        } else {
            backend.disable(&s.autostart.id).map(|_| ())
        }
    })
    .await
    .map_err(|e| RpcError::internal(format!("autostart task failed: {e}")))?
    .map_err(|e| platform_to_rpc("could not change autostart", e))?;
    tracing::info!(enabled, id = %services.autostart.id, "autostart registration changed");
    Ok(())
}

async fn get(ctx: RpcContext) -> Result<SettingsResult, RpcError> {
    let services = &ctx.services;
    let _guard = services.settings.lock().await;
    let mut settings = services.settings.get();
    // The user may have removed the OS registration by hand: reflect reality.
    if let Some(actual) = autostart_registered(services).await {
        if actual != settings.autostart {
            settings.autostart = actual;
            services.settings.save(settings.clone(), &["autostart"]).await.map_err(storage_to_rpc)?;
        }
    }
    Ok(SettingsResult { settings, autostart_supported: autostart_supported() })
}

async fn update(ctx: RpcContext, p: UpdateSettingsParams) -> Result<SettingsResult, RpcError> {
    p.patch.validate().map_err(|e| RpcError::bad_request(e.to_string()))?;
    let services = &ctx.services;
    let _guard = services.settings.lock().await;
    let mut next: AppSettings = services.settings.get();
    let changed = p.patch.apply_to(&mut next);
    if changed.contains(&"autostart") {
        set_autostart(services, next.autostart).await?;
    }
    services.settings.save(next.clone(), &changed).await.map_err(storage_to_rpc)?;
    if !changed.is_empty() {
        tracing::info!(changed = ?changed, "settings updated");
        let payload = SettingsChangedPayload {
            settings: next.clone(),
            changed: changed.iter().map(|c| (*c).to_owned()).collect(),
        };
        services.events.publish(NewEvent::new(EventKind::SettingsChanged(payload))).await.map_err(events_to_rpc)?;
    }
    Ok(SettingsResult { settings: next, autostart_supported: autostart_supported() })
}
