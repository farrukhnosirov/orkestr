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
#![forbid(unsafe_code)]

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

/// Label of the main window (see `tauri.conf.json`).
const MAIN_WINDOW: &str = "main";
/// Command-line flag used by the autostart entry: start hidden in the tray.
pub const AUTOSTART_FLAG: &str = "--autostart";
/// Event emitted to the webview when the tray's *Stop All AI* is clicked.
pub const TRAY_STOP_ALL_EVENT: &str = "orkestr://tray/stop-all";

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

fn quit(app: &AppHandle) {
    app.state::<ShellState>().quitting.store(true, Ordering::Release);
    app.exit(0);
}

fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    let toggle = MenuItem::with_id(app, "toggle", "Show / Hide Orkestr", true, None::<&str>)?;
    let stop_all = MenuItem::with_id(app, "stop_all", "Stop All AI", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit Orkestr", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&toggle, &stop_all, &separator, &quit_item])?;
    let mut builder = TrayIconBuilder::with_id("orkestr-tray")
        .tooltip("Orkestr — AI agents keep running in the background")
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
        .manage(ShellState { close_to_tray: AtomicBool::new(true), quitting: AtomicBool::new(false) })
        .setup(move |app| {
            if let Err(e) = build_tray(app) {
                // Without a tray, hiding would strand the user: disable close-to-tray.
                tracing::error!(error = %e, "failed to create the system tray icon");
                app.state::<ShellState>().close_to_tray.store(false, Ordering::Release);
            }
            if start_hidden {
                tracing::info!("started by autostart; staying in the tray");
            } else if let Some(w) = app.get_webview_window(MAIN_WINDOW) {
                let _ = w.show();
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
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let state = window.app_handle().state::<ShellState>();
                if window.label() == MAIN_WINDOW
                    && state.close_to_tray.load(Ordering::Acquire)
                    && !state.quitting.load(Ordering::Acquire)
                {
                    api.prevent_close();
                    let _ = window.hide();
                    tracing::info!("main window hidden to tray; agents keep running");
                }
            }
        })
        .invoke_handler(tauri::generate_handler![core_connection, shell_set_prefs, shell_quit]);

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
