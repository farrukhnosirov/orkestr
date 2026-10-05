//! `ui.*` handlers — validated UI config (global + per-device overrides) and
//! the theme library (§29, ADR-0015; Phase 3 theme engine).
//!
//! The core is the only trusted validator: every document is checked with
//! `UiConfig::validate` / `ThemeConfig::validate` (token grammars, ranges,
//! WCAG hard invariants) before it is stored. Writes are serialized by one
//! lock; changes publish `UiConfigChanged` / `UiThemesChanged` on `global`.
//!
//! The performance mode has a single source of truth: `AppSettings`
//! (`settings.*`). `ui.getConfig` reports it inside the config and
//! `ui.setConfig` forwards a changed value to the settings store.

use std::sync::{Arc, OnceLock};

use orkestr_protocol::event::{EventKind, SettingsChangedPayload, UiConfigChangedPayload, UiThemesChangedPayload};
use orkestr_protocol::methods::ui::{
    self as m, is_valid_device_id, DeleteThemeParams, DeleteThemeResult, GetUiConfigParams, SaveThemeParams,
    SaveThemeResult, SetUiConfigParams, ThemeEntry, ThemesResult, UiConfigResult, UiConfigScope,
};
use orkestr_protocol::ui_config::{ThemeConfig, UiConfig, UiConfigIssue};
use orkestr_protocol::ui_theme::{builtin_themes, contrast_report, is_builtin_theme, MAX_USER_THEMES};
use orkestr_protocol::{NewEvent, RpcError};

use super::{RpcContext, RpcRouter};
use crate::error::{events_to_rpc, storage_to_rpc};
use crate::services::Services;
use crate::ui_store::{self as store, device_scope, GLOBAL_SCOPE};

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::GetConfig, _, _>(get_config)
        .method::<m::SetConfig, _, _>(set_config)
        .method::<m::Themes, _, _>(|ctx: RpcContext, _| themes(ctx))
        .method::<m::SaveTheme, _, _>(save_theme)
        .method::<m::DeleteTheme, _, _>(delete_theme);
}

/// Serializes read-modify-write cycles of the UI store (one per process; the
/// store is tiny and writes are user-driven).
fn write_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn issues_to_rpc(issues: &[UiConfigIssue]) -> RpcError {
    let text = issues.iter().map(|i| format!("{}: {}", i.path, i.message)).collect::<Vec<_>>().join("; ");
    RpcError::bad_request(format!("invalid UI config: {text}"))
        .with_data(serde_json::to_value(issues).unwrap_or(serde_json::Value::Null))
}

fn check_device(id: Option<&String>) -> Result<(), RpcError> {
    match id {
        Some(d) if !is_valid_device_id(d) => Err(RpcError::bad_request("deviceId must match ^[A-Za-z0-9-]{1,64}$")),
        _ => Ok(()),
    }
}

/// The global document; first start seeds the theme from the legacy
/// `AppSettings.themeId` when it names a built-in theme.
async fn global(services: &Arc<Services>) -> Result<(UiConfig, u64), RpcError> {
    if let Some(s) = store::get_config(&services.db, GLOBAL_SCOPE).await.map_err(storage_to_rpc)? {
        return Ok((s.config, s.version));
    }
    let mut cfg = UiConfig::default();
    let legacy = services.settings.get().theme_id;
    if let Some(t) = builtin_themes().into_iter().find(|t| t.id == legacy) {
        cfg.theme = t;
    } else if let Some(t) = builtin_themes().into_iter().next() {
        cfg.theme = t;
    }
    let version = store::version_of(&services.db, GLOBAL_SCOPE).await.map_err(storage_to_rpc)?;
    Ok((cfg, version))
}

async fn effective(services: &Arc<Services>, device_id: Option<&str>) -> Result<UiConfigResult, RpcError> {
    let (mut config, version, scope) = match device_id {
        Some(d) => match store::get_config(&services.db, &device_scope(d)).await.map_err(storage_to_rpc)? {
            Some(s) => (s.config, s.version, UiConfigScope::Device),
            None => {
                let (c, v) = global(services).await?;
                (c, v, UiConfigScope::Global)
            }
        },
        None => {
            let (c, v) = global(services).await?;
            (c, v, UiConfigScope::Global)
        }
    };
    config.performance_mode = services.settings.get().performance_mode;
    let contrast = contrast_report(&config.theme);
    Ok(UiConfigResult { config, version, scope, contrast })
}

async fn get_config(ctx: RpcContext, p: GetUiConfigParams) -> Result<UiConfigResult, RpcError> {
    check_device(p.device_id.as_ref())?;
    effective(&ctx.services, p.device_id.as_deref()).await
}

async fn publish(services: &Arc<Services>, kind: EventKind) -> Result<(), RpcError> {
    services.events.publish(NewEvent::new(kind)).await.map(|_| ()).map_err(events_to_rpc)
}

/// Forwards a changed performance mode to the settings store (single source of truth).
async fn sync_performance_mode(services: &Arc<Services>, config: &UiConfig) -> Result<(), RpcError> {
    let _guard = services.settings.lock().await;
    let mut next = services.settings.get();
    if next.performance_mode == config.performance_mode {
        return Ok(());
    }
    next.performance_mode = config.performance_mode;
    services.settings.save(next.clone(), &["performanceMode"]).await.map_err(storage_to_rpc)?;
    let payload = SettingsChangedPayload { settings: next, changed: vec!["performanceMode".into()] };
    publish(services, EventKind::SettingsChanged(payload)).await
}

async fn set_config(ctx: RpcContext, p: SetUiConfigParams) -> Result<UiConfigResult, RpcError> {
    check_device(p.device_id.as_ref())?;
    let services = &ctx.services;
    let scope = p.device_id.as_deref().map_or_else(|| GLOBAL_SCOPE.to_owned(), device_scope);
    let _guard = write_lock().lock().await;
    let current = store::version_of(&services.db, &scope).await.map_err(storage_to_rpc)?;
    if let Some(base) = p.base_version {
        if base != current {
            return Err(RpcError::conflict(format!("UI config changed (version {current}, expected {base})")));
        }
    }
    match &p.config {
        Some(config) => {
            config.validate().map_err(|i| issues_to_rpc(&i))?;
            let version = store::put_config(&services.db, &scope, config).await.map_err(storage_to_rpc)?;
            tracing::info!(scope = %scope, version, theme = %config.theme.id, "UI config updated");
            sync_performance_mode(services, config).await?;
            let payload = UiConfigChangedPayload { device_id: p.device_id.clone(), config: Some(config.clone()), version };
            publish(services, EventKind::UiConfigChanged(payload)).await?;
        }
        None => {
            if p.device_id.is_none() {
                return Err(RpcError::bad_request("config is required for the global scope"));
            }
            if store::delete_config(&services.db, &scope).await.map_err(storage_to_rpc)? {
                tracing::info!(scope = %scope, "UI device override removed");
                let payload = UiConfigChangedPayload { device_id: p.device_id.clone(), config: None, version: 0 };
                publish(services, EventKind::UiConfigChanged(payload)).await?;
            }
        }
    }
    effective(services, p.device_id.as_deref()).await
}

async fn themes(ctx: RpcContext) -> Result<ThemesResult, RpcError> {
    let mut themes: Vec<ThemeEntry> =
        builtin_themes().into_iter().map(|theme| ThemeEntry { theme, builtin: true, updated_at: None }).collect();
    themes.extend(store::user_themes(&ctx.services.db).await.map_err(storage_to_rpc)?);
    Ok(ThemesResult { themes })
}

/// Applies `f` to every stored config whose theme id is `id`; returns how many changed.
async fn rewrite_configs_using(
    services: &Arc<Services>,
    id: &str,
    f: impl Fn(&mut UiConfig),
) -> Result<u32, RpcError> {
    let mut n = 0u32;
    for (scope, stored) in store::all_configs(&services.db).await.map_err(storage_to_rpc)? {
        if stored.config.theme.id != id {
            continue;
        }
        let mut cfg = stored.config;
        f(&mut cfg);
        if cfg.validate().is_err() {
            continue;
        }
        let version = store::put_config(&services.db, &scope, &cfg).await.map_err(storage_to_rpc)?;
        let device_id = scope.strip_prefix("device:").map(str::to_owned);
        publish(services, EventKind::UiConfigChanged(UiConfigChangedPayload { device_id, config: Some(cfg), version }))
            .await?;
        n += 1;
    }
    Ok(n)
}

async fn save_theme(ctx: RpcContext, p: SaveThemeParams) -> Result<SaveThemeResult, RpcError> {
    let services = &ctx.services;
    let theme: ThemeConfig = p.theme;
    theme.validate().map_err(|i| issues_to_rpc(&i))?;
    if is_builtin_theme(&theme.id) {
        return Err(RpcError::bad_request(format!(
            "`{}` is a built-in theme and read-only; duplicate it under a new id",
            theme.id
        )));
    }
    let _guard = write_lock().lock().await;
    let exists = store::theme_exists(&services.db, &theme.id).await.map_err(storage_to_rpc)?;
    if exists && !p.overwrite {
        return Err(RpcError::conflict(format!("a theme with id `{}` already exists", theme.id)));
    }
    if !exists && store::theme_count(&services.db).await.map_err(storage_to_rpc)? >= MAX_USER_THEMES {
        return Err(RpcError::bad_request(format!("at most {MAX_USER_THEMES} user themes")));
    }
    let entry = store::put_theme(&services.db, &theme).await.map_err(storage_to_rpc)?;
    let configs_updated = rewrite_configs_using(services, &theme.id, |cfg| cfg.theme = theme.clone()).await?;
    tracing::info!(theme = %theme.id, configs_updated, "theme saved");
    publish(services, EventKind::UiThemesChanged(UiThemesChangedPayload { theme_id: theme.id.clone(), deleted: false }))
        .await?;
    Ok(SaveThemeResult { contrast: contrast_report(&theme), entry, configs_updated })
}

async fn delete_theme(ctx: RpcContext, p: DeleteThemeParams) -> Result<DeleteThemeResult, RpcError> {
    let services = &ctx.services;
    if is_builtin_theme(&p.id) {
        return Err(RpcError::forbidden(format!("`{}` is a built-in theme and cannot be deleted", p.id)));
    }
    let _guard = write_lock().lock().await;
    let deleted = store::delete_theme(&services.db, &p.id).await.map_err(storage_to_rpc)?;
    if !deleted {
        return Ok(DeleteThemeResult { deleted: false, configs_reset: 0 });
    }
    let fallback = builtin_themes().into_iter().next().unwrap_or_else(|| UiConfig::default().theme);
    let configs_reset = rewrite_configs_using(services, &p.id, |cfg| cfg.theme = fallback.clone()).await?;
    tracing::info!(theme = %p.id, configs_reset, "theme deleted");
    publish(services, EventKind::UiThemesChanged(UiThemesChangedPayload { theme_id: p.id.clone(), deleted: true }))
        .await?;
    Ok(DeleteThemeResult { deleted: true, configs_reset })
}
