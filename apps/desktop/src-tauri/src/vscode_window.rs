//! The VS Code window: shows the embedded VS Code server's web UI
//! (`orkestr-vscode-host`, started over RPC with `vscode.start`).
//!
//! **Why a dedicated window instead of an `<iframe>` in the panel:** the
//! server swaps the `?tkn=` connection token for a `SameSite=Lax` cookie on
//! the first request. Inside the main webview (`tauri.localhost`) an iframe
//! on `127.0.0.1` is a cross-site frame, so the cookie is never sent with its
//! sub-requests and every asset fails with 403. Widening the main window CSP
//! (`frame-src http://127.0.0.1:*`) would also let any local web server be
//! framed into the privileged Orkestr UI. A separate top-level webview gets a
//! first-party cookie, keeps VS Code keyboard shortcuts (Ctrl+P,
//! Ctrl+Shift+P, …) away from Orkestr's own bindings, and — having no Tauri
//! capability — no IPC access at all.
//!
//! Hardening (all tested below):
//! * only `http://127.0.0.1:<port>` URLs are accepted ([`check_vscode_url`]);
//! * navigation is pinned to that origin; other `http(s)` links (docs,
//!   Open VSX pages, extension links) open in the system browser, anything
//!   else is blocked; `window.open` never creates an app window;
//! * the webview may read the clipboard (VS Code paste) — every other
//!   permission (camera, microphone, location, …) is denied.
//!
//! Window management only — the URL and token come from the core over RPC.

use std::sync::Mutex;

use serde::Serialize;
use tauri::webview::{NewWindowResponse, PermissionKind, PermissionResponse};
use tauri::{AppHandle, Emitter, Manager, Url, WebviewUrl, WebviewWindowBuilder, WindowEvent};

/// Label of the VS Code window. No capability matches it: the remote page
/// cannot call any Tauri command.
pub const VSCODE_WINDOW: &str = "vscode";
/// Emitted to the main window when the VS Code window opens or closes
/// (payload [`VsCodeWindowEvent`]).
pub const EVT_VSCODE_WINDOW: &str = "orkestr://vscode/window";

/// Payload of [`EVT_VSCODE_WINDOW`].
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VsCodeWindowEvent {
    /// Whether the window is open now.
    pub open: bool,
}

/// Origin the VS Code window is allowed to show (`http://127.0.0.1:<port>`).
#[derive(Default)]
pub struct VsCodeWindowState {
    origin: Mutex<Option<String>>,
}

/// Validates a VS Code UI URL: `http://127.0.0.1:<port>/…` and nothing else.
/// Returns its origin.
pub fn check_vscode_url(raw: &str) -> Result<String, String> {
    let url = Url::parse(raw).map_err(|e| format!("invalid VS Code URL: {e}"))?;
    if url.scheme() != "http" {
        return Err("the VS Code URL must use http on the loopback interface".into());
    }
    if url.host_str() != Some("127.0.0.1") {
        return Err("the VS Code URL must point to 127.0.0.1".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("the VS Code URL must not carry credentials".into());
    }
    let port = url.port().ok_or_else(|| "the VS Code URL has no port".to_owned())?;
    Ok(format!("http://127.0.0.1:{port}"))
}

/// What to do with a navigation of the VS Code window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavDecision {
    /// Same origin (or an in-page `about:` / `blob:` / `data:` document): allow.
    Allow,
    /// External web link: open in the system browser instead.
    External,
    /// Anything else: block.
    Block,
}

/// Navigation policy of the VS Code window.
#[must_use]
pub fn navigation_decision(origin: &str, target: &Url) -> NavDecision {
    match target.scheme() {
        "about" | "blob" | "data" => NavDecision::Allow,
        "http" | "https" => {
            let target_origin = target.origin().ascii_serialization();
            if target_origin == origin {
                NavDecision::Allow
            } else {
                NavDecision::External
            }
        }
        _ => NavDecision::Block,
    }
}

fn open_external(url: &Url) {
    if let Err(e) = orkestr_platform::open_url(url.as_str()) {
        tracing::warn!(error = %e, "could not open a VS Code link in the system browser");
    }
}

/// Opens (or focuses and re-targets) the VS Code window on `url`. Async: on
/// Windows a window built from a synchronous command (main thread) deadlocks
/// its first navigation.
#[tauri::command]
pub async fn vscode_window_open(
    app: AppHandle,
    state: tauri::State<'_, VsCodeWindowState>,
    url: String,
    title: Option<String>,
) -> Result<(), String> {
    let origin = check_vscode_url(&url)?;
    let parsed = Url::parse(&url).map_err(|e| e.to_string())?;
    let title = title.unwrap_or_else(|| "VS Code".to_owned()).chars().take(120).collect::<String>();
    *state.origin.lock().map_err(|_| "VS Code window state poisoned".to_owned())? = Some(origin.clone());
    if let Some(w) = app.get_webview_window(VSCODE_WINDOW) {
        // Same server (or a restarted one): show the requested folder.
        w.navigate(parsed).map_err(|e| e.to_string())?;
        let _ = w.set_title(&title);
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return Ok(());
    }
    let nav_app = app.clone();
    let window = WebviewWindowBuilder::new(&app, VSCODE_WINDOW, WebviewUrl::External(parsed))
        .title(&title)
        .inner_size(1360.0, 860.0)
        .min_inner_size(640.0, 420.0)
        .center()
        .theme(Some(tauri::Theme::Dark))
        .focused(true)
        .on_navigation(move |target| {
            let origin = nav_app
                .try_state::<VsCodeWindowState>()
                .and_then(|s| s.origin.lock().ok().and_then(|o| o.clone()))
                .unwrap_or_default();
            match navigation_decision(&origin, target) {
                NavDecision::Allow => true,
                NavDecision::External => {
                    open_external(target);
                    false
                }
                NavDecision::Block => {
                    tracing::warn!(scheme = target.scheme(), "blocked a navigation of the VS Code window");
                    false
                }
            }
        })
        .on_new_window(|target, _features| {
            if matches!(target.scheme(), "http" | "https") {
                open_external(&target);
            }
            NewWindowResponse::Deny
        })
        .on_permission_request(|_webview, kind| match kind {
            PermissionKind::ClipboardRead => PermissionResponse::Allow,
            _ => PermissionResponse::Deny,
        })
        .build()
        .map_err(|e| format!("could not open the VS Code window: {e}"))?;
    let events_app = app.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::Destroyed = event {
            let _ = events_app.emit_to("main", EVT_VSCODE_WINDOW, VsCodeWindowEvent { open: false });
        }
    });
    let _ = app.emit_to("main", EVT_VSCODE_WINDOW, VsCodeWindowEvent { open: true });
    Ok(())
}

/// Closes the VS Code window (no-op when it is not open).
#[tauri::command]
pub async fn vscode_window_close(app: AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(VSCODE_WINDOW) {
        w.destroy().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Reloads the VS Code window.
#[tauri::command]
pub async fn vscode_window_reload(app: AppHandle) -> Result<(), String> {
    match app.get_webview_window(VSCODE_WINDOW) {
        Some(w) => w.reload().map_err(|e| e.to_string()),
        None => Err("the VS Code window is not open".into()),
    }
}

/// Whether the VS Code window is open.
#[tauri::command]
pub fn vscode_window_is_open(app: AppHandle) -> bool {
    app.get_webview_window(VSCODE_WINDOW).is_some()
}

/// Opens the VS Code UI (loopback URL only) in the system browser.
#[tauri::command]
pub fn vscode_open_external(url: String) -> Result<(), String> {
    check_vscode_url(&url)?;
    orkestr_platform::open_url(&url).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loopback_http_urls_are_accepted() {
        assert_eq!(check_vscode_url("http://127.0.0.1:9900/?tkn=abc&folder=/c:/x").unwrap(), "http://127.0.0.1:9900");
        for bad in [
            "https://127.0.0.1:9900/",
            "http://localhost:9900/",
            "http://0.0.0.0:9900/",
            "http://127.0.0.2:9900/",
            "http://evil.example/",
            "http://user:pw@127.0.0.1:9900/",
            "file:///C:/x",
            "javascript:alert(1)",
            "not a url",
        ] {
            assert!(check_vscode_url(bad).is_err(), "{bad} must be rejected");
        }
    }

    #[test]
    fn navigation_is_pinned_to_the_server_origin() {
        let origin = "http://127.0.0.1:9900";
        let d = |u: &str| navigation_decision(origin, &Url::parse(u).unwrap());
        assert_eq!(d("http://127.0.0.1:9900/?folder=/c:/x"), NavDecision::Allow);
        assert_eq!(d("about:blank"), NavDecision::Allow);
        assert_eq!(d("https://open-vsx.org/extension/redhat/vscode-yaml"), NavDecision::External);
        assert_eq!(d("http://127.0.0.1:1234/"), NavDecision::External, "another local server is not VS Code");
        assert_eq!(d("file:///C:/Windows/win.ini"), NavDecision::Block);
        assert_eq!(d("vscode://settings"), NavDecision::Block);
        assert_eq!(navigation_decision("", &Url::parse("http://127.0.0.1:9900/").unwrap()), NavDecision::External);
    }
}
