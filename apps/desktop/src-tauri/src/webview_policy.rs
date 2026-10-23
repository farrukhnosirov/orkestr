//! Web view settings of Orkestr's own windows: the WebView2 default context
//! menu is switched off (Orkestr renders its own) and the bundled UI may read
//! the clipboard without an Edge permission prompt
//! (`orkestr_platform::webview_shell`).
//!
//! Applied by a tiny internal plugin to every web view as it is created —
//! the main window from `tauri.conf.json`, detached panels, the HUD and the
//! orb window — except the VS Code window, which shows a third-party
//! web UI that keeps its own menus.

use tauri::plugin::{Builder, TauriPlugin};
use tauri::{Runtime, Webview};

use crate::vscode_window::VSCODE_WINDOW;

/// Whether the policy applies to the web view with this label.
pub fn applies_to(label: &str) -> bool {
    label != VSCODE_WINDOW
}

/// The plugin applying the policy (register it before windows are created).
pub fn plugin<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("orkestr-webview-policy").on_webview_ready(|webview| apply(&webview)).build()
}

fn apply<R: Runtime>(webview: &Webview<R>) {
    if !applies_to(webview.label()) {
        return;
    }
    let label = webview.label().to_owned();
    let res = webview.with_webview(move |platform| {
        // `controller()` exists on Windows only; other engines rely on the
        // UI's global `contextmenu` handler.
        #[cfg(windows)]
        if let Err(e) = orkestr_platform::webview_shell::webview2::apply_app_webview_policy(&platform.controller()) {
            tracing::warn!(window = %label, "could not apply the web view policy: {e}");
        }
        #[cfg(not(windows))]
        let _ = (platform, label);
    });
    if let Err(e) = res {
        tracing::warn!("with_webview failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_skips_only_the_vscode_window() {
        assert!(applies_to("main"));
        assert!(applies_to("hud"));
        assert!(applies_to("panel-files"));
        assert!(applies_to("orb"));
        assert!(!applies_to(VSCODE_WINDOW));
    }
}
