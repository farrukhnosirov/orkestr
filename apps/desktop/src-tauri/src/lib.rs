//! Orkestr desktop shell (Tauri 2).
//!
//! On startup the shell either connects to an already running `orkestrd`
//! (discovered through `<data_dir>/core.json`) or starts the core **embedded**
//! in this process (contract §3). The webview obtains `{url, token}` through
//! the `core_connection` command and from then on talks to the core only via
//! the RPC protocol — no business data flows through Tauri commands (§67).
//!
//! Background mode (§12, ADR-0004):
//! * **System tray** — show/hide the window, *Stop All AI* (forwarded to the
//!   webview, which calls `emergency.stopAll` over RPC), *Quit*.
//! * **Close to tray** — closing the window hides it so agents keep running;
//!   the webview mirrors the `closeToTray` setting via `shell_set_prefs`.
//! * **Autostart** — registered by the core (`settings.update {autostart}`)
//!   as `<this exe> --autostart`; such launches start hidden in the tray.
//! * **Single instance** — a second launch focuses the existing window.
//!
//! HUD / Ambient (§34, Phase 3): a compact always-on-top window, see [`hud`].
//!
//! Localization: the webview reports the resolved UI language through
//! `set_ui_language` (at startup and on every switch); the tray menu and
//! tooltip are rebuilt in that language (see [`tray_texts`]).
#![forbid(unsafe_code)]

mod hud;
pub mod windows;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use orkestr_core::{AutostartConfig, CoreConfig, CoreHandle};
use orkestr_platform::Paths;
use serde::Serialize;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent};
use tokio::sync::{watch, Mutex};

pub mod robots;

/// Label of the main window (see `tauri.conf.json`).
const MAIN_WINDOW: &str = "main";
/// Command-line flag used by the autostart entry: start hidden in the tray.
pub const AUTOSTART_FLAG: &str = "--autostart";
/// Event emitted to the webview when the tray's *Stop All AI* is clicked.
pub const TRAY_STOP_ALL_EVENT: &str = "orkestr://tray/stop-all";
/// Id of the system tray icon.
const TRAY_ID: &str = "orkestr-tray";

/// Texts of the native tray (the only UI the shell draws itself).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrayTexts {
    /// Show / hide the main window.
    pub toggle: &'static str,
    /// Emergency stop of all AI activity.
    pub stop_all: &'static str,
    /// Quit the app (and the embedded core).
    pub quit: &'static str,
    /// Tray icon tooltip.
    pub tooltip: &'static str,
}

/// Tray texts for a UI language (`ru`, `en`, `ru-RU`…); English otherwise.
/// Terminology follows docs/i18n/glossary.md.
#[must_use]
pub fn tray_texts(language: &str) -> TrayTexts {
    let primary = language.split(['-', '_']).next().unwrap_or_default().to_ascii_lowercase();
    match primary.as_str() {
        "ru" => TrayTexts {
            toggle: "Показать или скрыть Orkestr",
            stop_all: "Остановить всю работу AI",
            quit: "Выйти из Orkestr",
            tooltip: "Orkestr — AI-агенты продолжают работать в фоне",
        },
        _ => TrayTexts {
            toggle: "Show / Hide Orkestr",
            stop_all: "Stop All AI",
            quit: "Quit Orkestr",
            tooltip: "Orkestr — AI agents keep running in the background",
        },
    }
}

/// Connection info handed to the webview.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreConnection {
    /// RPC WebSocket URL.
    pub url: String,
    /// Bearer token.
    pub token: String,
    /// `embedded` or `external` (an already running `orkestrd`).
    pub mode: &'static str,
    /// Milliseconds it took to get the core ready.
    pub startup_ms: u64,
}

type Ready = Option<Result<CoreConnection, String>>;

struct CoreState {
    ready: watch::Receiver<Ready>,
    handle: Arc<Mutex<Option<CoreHandle>>>,
}

/// Window/tray behaviour mirrored from the core settings by the webview.
struct ShellState {
    close_to_tray: AtomicBool,
    quitting: AtomicBool,
    /// Language the tray is currently shown in.
    language: std::sync::Mutex<String>,
}

async fn boot_core(handle_slot: Arc<Mutex<Option<CoreHandle>>>) -> Result<CoreConnection, String> {
    let started = Instant::now();
    let paths = Paths::resolve().map_err(|e| e.to_string())?;
    if let Some(running) = orkestr_core::discovery::find_running(&paths).await {
        orkestr_core::redact::register_secret(&running.token);
        tracing::info!(url = %running.url, pid = running.pid, "connecting to running orkestrd");
        return Ok(CoreConnection {
            url: running.url,
            token: running.token,
            mode: "external",
            startup_ms: started.elapsed().as_millis() as u64,
        });
    }
    let config = CoreConfig {
        autostart: AutostartConfig {
            id: "dev.orkestr.desktop".into(),
            args: vec![AUTOSTART_FLAG.into()],
            ..AutostartConfig::default()
        },
        ..CoreConfig::default()
    };
    let core = orkestr_core::start(config).await.map_err(|e| e.to_string())?;
    let conn = CoreConnection {
        url: core.url.clone(),
        token: core.token.clone(),
        mode: "embedded",
        startup_ms: started.elapsed().as_millis() as u64,
    };
    tracing::info!(url = %conn.url, startup_ms = conn.startup_ms, "embedded core started");
    *handle_slot.lock().await = Some(core);
    Ok(conn)
}

/// Returns `{url, token}` of the core, waiting (up to 30 s) until it is ready.
#[tauri::command]
async fn core_connection(state: tauri::State<'_, CoreState>) -> Result<CoreConnection, String> {
    let mut rx = state.ready.clone();
    let ready = tokio::time::timeout(Duration::from_secs(30), rx.wait_for(Option::is_some))
        .await
        .map_err(|_| "timed out waiting for the Orkestr core to start".to_owned())?
        .map_err(|_| "core startup task ended unexpectedly".to_owned())?;
    match ready.as_ref() {
        Some(result) => result.clone(),
        None => Err("core not ready".to_owned()),
    }
}

/// Shell preferences mirrored from `AppSettings` (window behaviour only).
#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShellPrefs {
    close_to_tray: bool,
}

/// Applies window behaviour preferences (not business data: §67 allows window management).
#[tauri::command]
fn shell_set_prefs(prefs: ShellPrefs, state: tauri::State<'_, ShellState>) {
    state.close_to_tray.store(prefs.close_to_tray, Ordering::Release);
}

/// Quits the app (and the embedded core) even when close-to-tray is on.
#[tauri::command]
fn shell_quit(app: AppHandle) {
    quit(&app);
}

/// Localizes the native shell (tray menu + tooltip) to the UI language the
/// webview resolved (`ru`, `en`). Window management only — no business data.
#[tauri::command]
fn set_ui_language(language: String, app: AppHandle, state: tauri::State<'_, ShellState>) -> Result<(), String> {
    let texts = tray_texts(&language);
    {
        let mut current = state.language.lock().map_err(|_| "shell state poisoned".to_owned())?;
        if *current == language {
            return Ok(());
        }
        current.clone_from(&language);
    }
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let menu = tray_menu(&app, texts).map_err(|e| e.to_string())?;
        tray.set_menu(Some(menu)).map_err(|e| e.to_string())?;
        tray.set_tooltip(Some(texts.tooltip)).map_err(|e| e.to_string())?;
    }
    tracing::info!(%language, "shell language applied");
    Ok(())
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(MAIN_WINDOW) {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn toggle_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(MAIN_WINDOW) {
        let visible = w.is_visible().unwrap_or(false);
        let minimized = w.is_minimized().unwrap_or(false);
        if visible && !minimized {
            let _ = w.hide();
        } else {
            show_main(app);
        }
    }
}

/// What happens when the main window's close button is pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MainCloseAction {
    /// Hide to the tray; agents keep running.
    HideToTray,
    /// Quit the whole app: auxiliary windows (HUD, robots, detached panels)
    /// must not keep a headless process alive once the main window is gone.
    Quit,
    /// Already quitting: let the window close.
    Proceed,
}

fn main_close_action(close_to_tray: bool, quitting: bool) -> MainCloseAction {
    if quitting {
        MainCloseAction::Proceed
    } else if close_to_tray {
        MainCloseAction::HideToTray
    } else {
        MainCloseAction::Quit
    }
}

/// Whether the main window is shown at startup. Autostart launches stay in
/// the tray — but only when there *is* a tray to get the window back from.
fn show_main_on_startup(start_hidden: bool, tray_ok: bool) -> bool {
    !(start_hidden && tray_ok)
}

fn quit(app: &AppHandle) {
    app.state::<ShellState>().quitting.store(true, Ordering::Release);
    app.exit(0);
}

fn tray_menu<R: tauri::Runtime, M: Manager<R>>(app: &M, texts: TrayTexts) -> tauri::Result<Menu<R>> {
    let toggle = MenuItem::with_id(app, "toggle", texts.toggle, true, None::<&str>)?;
    let stop_all = MenuItem::with_id(app, "stop_all", texts.stop_all, true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(app, "quit", texts.quit, true, None::<&str>)?;
    Menu::with_items(app, &[&toggle, &stop_all, &separator, &quit_item])
}

fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    // English until the webview reports the UI language (`set_ui_language`, sent right after it loads).
    let texts = tray_texts("en");
    let menu = tray_menu(app, texts)?;
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip(texts.tooltip)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "toggle" => toggle_main(app),
            "stop_all" => {
                tracing::warn!("Stop All AI requested from the tray");
                if let Err(e) = app.emit_to(MAIN_WINDOW, TRAY_STOP_ALL_EVENT, ()) {
                    tracing::error!(error = %e, "failed to forward Stop All AI to the UI");
                }
            }
            "quit" => quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                toggle_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

/// Entry point used by `main.rs` (and mobile targets in the future).
pub fn run() {
    orkestr_core::logging::init(false);
    let start_hidden = std::env::args().any(|a| a == AUTOSTART_FLAG);
    let (ready_tx, ready_rx) = watch::channel::<Ready>(None);
    let handle_slot: Arc<Mutex<Option<CoreHandle>>> = Arc::new(Mutex::new(None));

    let builder = tauri::Builder::default()
        // Must be the first plugin: a second launch focuses this instance and exits.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tracing::info!("second launch detected; focusing the existing window");
            show_main(app);
        }))
        .manage(CoreState { ready: ready_rx, handle: Arc::clone(&handle_slot) })
        .manage(ShellState {
            close_to_tray: AtomicBool::new(true),
            quitting: AtomicBool::new(false),
            language: std::sync::Mutex::new("en".to_owned()),
        })
        .manage(robots::RobotsState::default())
        .setup(move |app| {
            let tray_ok = match build_tray(app) {
                Ok(()) => true,
                Err(e) => {
                    // Without a tray, hiding would strand the user: disable close-to-tray.
                    tracing::error!(error = %e, "failed to create the system tray icon");
                    app.state::<ShellState>().close_to_tray.store(false, Ordering::Release);
                    false
                }
            };
            if show_main_on_startup(start_hidden, tray_ok) {
                if let Some(w) = app.get_webview_window(MAIN_WINDOW) {
                    let _ = w.show();
                    if start_hidden {
                        // Autostart without a tray: stay out of the way, but reachable.
                        tracing::warn!("started by autostart without a tray; showing the window minimized");
                        let _ = w.minimize();
                    }
                }
            } else {
                tracing::info!("started by autostart; staying in the tray");
            }
            let slot = Arc::clone(&handle_slot);
            tauri::async_runtime::spawn(async move {
                let result = boot_core(slot).await;
                if let Err(e) = &result {
                    tracing::error!(error = %e, "failed to start or find the Orkestr core");
                }
                let _ = ready_tx.send(Some(result));
            });
            Ok(())
        })
        // Detached panel windows / multi-monitor (Phase 3, `windows.rs`).
        .manage(windows::PanelWindows::default())
        .on_window_event(|window, event| {
            windows::on_panel_window_event(window, event);
            robots::on_window_event(window, event);
            if window.label() != MAIN_WINDOW {
                return;
            }
            let app = window.app_handle();
            let state = app.state::<ShellState>();
            let quitting = state.quitting.load(Ordering::Acquire);
            match event {
                WindowEvent::CloseRequested { api, .. } => {
                    match main_close_action(state.close_to_tray.load(Ordering::Acquire), quitting) {
                        MainCloseAction::HideToTray => {
                            api.prevent_close();
                            let _ = window.hide();
                            tracing::info!("main window hidden to tray; agents keep running");
                        }
                        MainCloseAction::Quit => {
                            // Auxiliary windows (HUD, robots, panels) would otherwise keep
                            // a headless, unreachable process alive.
                            api.prevent_close();
                            tracing::info!("main window closed with close-to-tray off; quitting");
                            quit(app);
                        }
                        MainCloseAction::Proceed => {}
                    }
                }
                // However the main window went away, the app cannot be reopened
                // without it: never linger headless.
                WindowEvent::Destroyed if !quitting => {
                    tracing::warn!("main window destroyed; quitting");
                    quit(app);
                }
                _ => {}
            }
        })
        .invoke_handler(tauri::generate_handler![
            core_connection,
            shell_set_prefs,
            shell_quit,
            set_ui_language,
            windows::window_list_monitors,
            windows::window_open_panel,
            windows::window_close_panel,
            windows::window_redock_panel,
            windows::window_list_panels,
            windows::window_main_placement,
            windows::window_move_to_monitor,
            windows::window_reconcile,
            hud::hud_show,
            hud::hud_hide,
            hud::hud_toggle,
            hud::hud_collapse,
            hud::hud_expand,
            robots::robot_open,
            robots::robot_close,
            robots::robot_close_all,
            robots::robot_set_shape,
            robots::robot_set_drop_zones,
            robots::robot_to_main
        ]);

    let app = match builder.build(tauri::generate_context!()) {
        Ok(app) => app,
        Err(e) => {
            tracing::error!(error = %e, "failed to build the Orkestr desktop app");
            eprintln!("failed to build the Orkestr desktop app: {e}");
            std::process::exit(1);
        }
    };

    app.run(|app_handle, event| {
        if let RunEvent::Exit = event {
            let state = app_handle.state::<CoreState>();
            let slot = Arc::clone(&state.handle);
            tauri::async_runtime::block_on(async move {
                if let Some(core) = slot.lock().await.take() {
                    core.shutdown().await;
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_main_without_tray_quits_instead_of_leaving_a_headless_process() {
        assert_eq!(main_close_action(true, false), MainCloseAction::HideToTray);
        assert_eq!(main_close_action(false, false), MainCloseAction::Quit);
        assert_eq!(main_close_action(true, true), MainCloseAction::Proceed);
        assert_eq!(main_close_action(false, true), MainCloseAction::Proceed);
    }

    #[test]
    fn autostart_stays_hidden_only_when_the_tray_exists() {
        assert!(!show_main_on_startup(true, true), "autostart with a tray stays in the tray");
        assert!(show_main_on_startup(true, false), "autostart without a tray must stay reachable");
        assert!(show_main_on_startup(false, true));
        assert!(show_main_on_startup(false, false));
    }

    #[test]
    fn tray_texts_follow_the_ui_language() {
        assert_eq!(tray_texts("ru").stop_all, "Остановить всю работу AI");
        assert_eq!(tray_texts("ru-RU").quit, "Выйти из Orkestr");
        assert_eq!(tray_texts("en").toggle, "Show / Hide Orkestr");
        assert_eq!(tray_texts("de"), tray_texts("en"), "unsupported languages fall back to English");
    }
}
