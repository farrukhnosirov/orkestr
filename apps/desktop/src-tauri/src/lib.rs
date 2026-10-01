//! Orkestr desktop shell (Tauri 2).
//!
//! On startup the shell either connects to an already running `orkestrd`
//! (discovered through `<data_dir>/core.json`) or starts the core **embedded**
//! in this process (contract §3). The webview obtains `{url, token}` through
//! the `core_connection` command and from then on talks to the core only via
//! the RPC protocol — no business data flows through Tauri commands (§67).
#![forbid(unsafe_code)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use orkestr_core::{CoreConfig, CoreHandle};
use orkestr_platform::Paths;
use serde::Serialize;
use tauri::{Manager, RunEvent};
use tokio::sync::{watch, Mutex};

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
    let core = orkestr_core::start(CoreConfig::default()).await.map_err(|e| e.to_string())?;
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

/// Entry point used by `main.rs` (and mobile targets in the future).
pub fn run() {
    orkestr_core::logging::init(false);
    let (ready_tx, ready_rx) = watch::channel::<Ready>(None);
    let handle_slot: Arc<Mutex<Option<CoreHandle>>> = Arc::new(Mutex::new(None));

    let app = tauri::Builder::default()
        .manage(CoreState { ready: ready_rx, handle: Arc::clone(&handle_slot) })
        .setup(move |_app| {
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
        .invoke_handler(tauri::generate_handler![core_connection])
        .build(tauri::generate_context!())
        .expect("failed to build the Orkestr desktop app");

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
