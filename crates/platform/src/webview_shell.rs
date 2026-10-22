//! Settings applied to the web views of Orkestr's own windows (main window,
//! detached panels, HUD, robot/orb windows).
//!
//! * **No built-in browser context menu.** Orkestr draws its own context menu
//!   (themed, localized, keyboard accessible — see `docs/adr/0013`); the
//!   WebView2 default menu ("Back", "Refresh", "Save as", "Inspect", Edge
//!   styling) is switched off with `AreDefaultContextMenusEnabled = false`.
//!   The web UI additionally cancels unhandled `contextmenu` events, so other
//!   engines (WKWebView, WebKitGTK) and dev builds behave the same.
//! * **Clipboard read for the app origin only.** The menu's "Paste" uses
//!   `navigator.clipboard.readText()`; WebView2 would otherwise show an Edge
//!   permission prompt. Only documents of a trusted origin (the bundled UI)
//!   are granted; plugin widget frames (`orkestr-widget:`) and anything else
//!   keep the default (prompt / deny).
//!
//! The pure origin check is portable and unit-tested; the COM calls live in
//! the Windows-only [`webview2`] module (feature `webview2`).

/// Whether `uri` belongs to one of the `trusted` origins
/// (`scheme://host[:port]`, compared case-insensitively). A prefix match
/// alone is not enough: `http://tauri.localhost.evil` is not
/// `http://tauri.localhost`.
#[must_use]
pub fn is_trusted_origin(uri: &str, trusted: &[&str]) -> bool {
    let uri = uri.to_ascii_lowercase();
    trusted.iter().any(|origin| {
        let origin = origin.trim_end_matches('/').to_ascii_lowercase();
        if origin.is_empty() {
            return false;
        }
        match uri.strip_prefix(origin.as_str()) {
            Some(rest) => rest.is_empty() || rest.starts_with(['/', '?', '#']),
            None => false,
        }
    })
}

/// Origins of the bundled Orkestr UI: the custom protocol on Windows/Android
/// (`http://tauri.localhost`), on macOS/Linux (`tauri://localhost`) and the
/// Vite dev server.
pub const APP_ORIGINS: &[&str] = &[
    "http://tauri.localhost",
    "https://tauri.localhost",
    "tauri://localhost",
    "http://127.0.0.1:1420",
    "http://localhost:1420",
];

/// WebView2 (Windows) implementation.
#[cfg(all(windows, feature = "webview2"))]
pub mod webview2 {
    // The only `unsafe` here: COM calls into the WebView2 runtime through the
    // `webview2-com` bindings (every WebView2 method is an `unsafe fn`). The
    // interface pointers come from the live controller Tauri hands out and are
    // used on the UI thread inside `with_webview`.
    #![allow(unsafe_code)]

    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2Controller, COREWEBVIEW2_PERMISSION_KIND, COREWEBVIEW2_PERMISSION_KIND_CLIPBOARD_READ,
        COREWEBVIEW2_PERMISSION_STATE_ALLOW,
    };
    use webview2_com::{take_pwstr, PermissionRequestedEventHandler};
    use windows_core::PWSTR;

    use super::{is_trusted_origin, APP_ORIGINS};

    /// Re-export so callers name the controller type without the bindings crate.
    pub type Controller = ICoreWebView2Controller;

    /// Disables the default context menus and grants clipboard read to the
    /// app origin (see the module docs).
    ///
    /// # Errors
    /// The WebView2 call that failed, as text.
    pub fn apply_app_webview_policy(controller: &ICoreWebView2Controller) -> Result<(), String> {
        unsafe {
            let webview = controller.CoreWebView2().map_err(|e| format!("CoreWebView2: {e}"))?;
            let settings = webview.Settings().map_err(|e| format!("Settings: {e}"))?;
            settings
                .SetAreDefaultContextMenusEnabled(false)
                .map_err(|e| format!("SetAreDefaultContextMenusEnabled: {e}"))?;
            let mut token: i64 = 0;
            webview
                .add_PermissionRequested(
                    &PermissionRequestedEventHandler::create(Box::new(|_, args| {
                        let Some(args) = args else { return Ok(()) };
                        let mut kind = COREWEBVIEW2_PERMISSION_KIND::default();
                        args.PermissionKind(&mut kind)?;
                        if kind != COREWEBVIEW2_PERMISSION_KIND_CLIPBOARD_READ {
                            return Ok(());
                        }
                        let mut uri = PWSTR::null();
                        args.Uri(&mut uri)?;
                        let uri = take_pwstr(uri);
                        if is_trusted_origin(&uri, APP_ORIGINS) {
                            args.SetState(COREWEBVIEW2_PERMISSION_STATE_ALLOW)?;
                        }
                        Ok(())
                    })),
                    &mut token,
                )
                .map_err(|e| format!("add_PermissionRequested: {e}"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trusted_origin_matches_exact_origin_and_paths() {
        assert!(is_trusted_origin("http://tauri.localhost", APP_ORIGINS));
        assert!(is_trusted_origin("http://tauri.localhost/", APP_ORIGINS));
        assert!(is_trusted_origin("http://tauri.localhost/index.html#/panel/x", APP_ORIGINS));
        assert!(is_trusted_origin("HTTP://Tauri.LocalHost/robot.html", APP_ORIGINS));
        assert!(is_trusted_origin("tauri://localhost/index.html", APP_ORIGINS));
        assert!(is_trusted_origin("http://127.0.0.1:1420/?x=1", APP_ORIGINS));
    }

    #[test]
    fn untrusted_origins_are_rejected() {
        assert!(!is_trusted_origin("http://tauri.localhost.evil.com/", APP_ORIGINS));
        assert!(!is_trusted_origin("http://orkestr-widget.localhost/w/1", APP_ORIGINS));
        assert!(!is_trusted_origin("orkestr-widget://localhost/w/1", APP_ORIGINS));
        assert!(!is_trusted_origin("http://127.0.0.1:14200/", APP_ORIGINS));
        assert!(!is_trusted_origin("http://127.0.0.1:8080/", APP_ORIGINS));
        assert!(!is_trusted_origin("", APP_ORIGINS));
        assert!(!is_trusted_origin("http://tauri.localhost", &[""]));
    }
}
