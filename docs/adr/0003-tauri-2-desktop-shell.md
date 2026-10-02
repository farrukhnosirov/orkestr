# ADR-0003: Tauri 2 as the desktop shell

- **Status:** Accepted
- **Date:** 2026-10-02
- **Phase:** 0–1 (shell), 3 (overlay windows)
- **Spec:** §2, §28, §30, §34, §46, §47, §61 (Tauri spike)
- **Spikes:** SP-01 (shell), SP-02 (Monaco), SP-04 (transparent windows)
- **Risks:** R-01

## Context

§2 sets the stack: Tauri shell, Rust core, React + TS UI. It also says that if Tauri cannot meet a critical requirement, an ADR and a spike come first. The critical desktop requirements are:

- frameless window with a custom title bar and controls (§28);
- transparency, blur and glow (§28);
- floating, transparent, always-on-top, click-through robot windows (§30);
- multi-monitor (§34);
- tray and global hotkeys (§33, §46);
- Windows x64/ARM64, macOS and Linux builds (§2, §47);
- low idle overhead, so AI work is not slowed (§31).

## Decision drivers

1. The Rust core runs in-process with no IPC bridge in a second language.
2. Memory and installer size (WebView2 is shared with the OS on Windows).
3. Window-level control: transparency, `set_ignore_cursor_events`, always-on-top, `skip_taskbar`.
4. Security: capability-based IPC allowlist (Tauri 2 permissions).
5. A mobile story (Tauri 2 mobile exists, but §2 prefers React Native; see ADR-0032 planned).

## Considered options

| Option | Verdict |
|---|---|
| **Tauri 2** | **Chosen.** Rust-native, small binaries, per-window transparency and decorations, `set_ignore_cursor_events`, tray, global-shortcut, single-instance, updater and window-state plugins, and capability permissions |
| Electron | Rejected. The core would need a Node↔Rust bridge (napi) or to run as a separate process anyway; ~150 MB baseline; Chromium per window hurts §31 with many robot windows |
| Native per-OS UI (WinUI / AppKit / GTK) | Rejected. Three UIs break §47 (90–95% shared code) and §67 (macOS without a UI rewrite) |
| Flutter desktop + Rust FFI | Rejected. Monaco and xterm.js are web-only, and web/desktop UI sharing is lost |
| Wails / Neutralino | Rejected. Go, or a weaker window API |

## Decision

Use **Tauri 2.x** (`tauri = "2"`, `@tauri-apps/api@^2`, `@tauri-apps/cli@^2`).

- **Main window:** `decorations: false`, `transparent: true` on Windows/macOS (blur/acrylic through the `window-vibrancy` crate behind a `PlatformAdapter` window helper), and a custom title bar component in `apps/desktop/src/shell/TitleBar.tsx` with `data-tauri-drag-region`.
- **Tauri commands are an allowlist** (RR-P5). Business data never flows through them:
  ```rust
  #[tauri::command] fn core_connection(state: State<CoreState>) -> CoreConnection; // {url, token}
  #[tauri::command] async fn window_open_overlay(app: AppHandle, spec: OverlaySpec) -> Result<String, String>; // Phase 3
  #[tauri::command] async fn window_set_click_through(win: Window, enabled: bool) -> Result<(), String>;
  #[tauri::command] async fn tray_set_status(app: AppHandle, status: TrayStatus) -> Result<(), String>;
  #[tauri::command] async fn dialog_pick_folder(app: AppHandle) -> Option<String>;   // a path picker is UI, not data
  ```
- **Capabilities file:** `apps/desktop/src-tauri/capabilities/default.json` grants only `core:default`, `window` operations and the specific plugin permissions. There is no `shell:allow-execute` and no `fs:` plugin, because file access goes through RPC with the permission engine.
- **Plugins:** `tauri-plugin-single-instance` (RR-G17), `tauri-plugin-window-state`, `tauri-plugin-global-shortcut` (Phase 3), `tauri-plugin-updater` (Phase 1 packaging, signed), `tauri-plugin-notification` (fallback; the primary path is `PlatformAdapter::notify`).
- **CSP:** `default-src 'self'; connect-src 'self' ws://127.0.0.1:* http://127.0.0.1:*; worker-src 'self' blob:; style-src 'self' 'unsafe-inline'` (Monaco needs inline styles); `img-src 'self' data: blob:`.
- **Webview runtimes:** WebView2 Evergreen (Windows, bootstrapper in the installer), WKWebView (macOS), WebKitGTK 4.1 (Linux).

## Consequences

- **Positive:** the core is embedded in the same process (ADR-0004), the installer is under 15 MB excluding WebView2, and transparent and click-through windows are first-class.
- **Negative:** three different webview engines mean CSS and WebGL differences (WebKitGTK is the weakest for `backdrop-filter` and WebGL2). Effects must degrade through Performance Mode (§31). See ADR-0015.
- **Negative:** Linux transparency needs a compositor. On Wayland, always-on-top and global position are not guaranteed, so Phase 3 has a fallback "docked robots" mode (ADR-0016).
- **Exit criteria for this ADR** (SP-01): frameless window with drag region and snap layouts on Win11 (`WM_NCHITTEST` maximize button behaviour noted); embedded core reachable from the webview; cold start under 1.5 s on the reference machine; idle CPU under 1%.

## Implementation notes (Phase 0 foundation, 2026-10-02)

Status and errata only; the decision above is unchanged. Verified by `../spikes/foundation-tauri-websocket.md` (partial SP-01).

- Tauri 2.12 (`orkestr-desktop`); main window `decorations: false`, `transparent: true`, `shadow: true`, 1440×900, min 960×600. No `window-vibrancy` yet: the app paints its own background.
- The title bar component is `apps/desktop/src/components/TitleBar.tsx`; window controls go through `apps/desktop/src/lib/tauri.ts` (`windowControls`, the only `@tauri-apps/*` import).
- The only custom command is `core_connection` → `{url, token, mode: "embedded" | "external", startupMs}`. `window_open_overlay`, `window_set_click_through`, `tray_set_status`, `dialog_pick_folder` are not implemented.
- No Tauri plugins are installed yet (single-instance, window-state, global-shortcut, updater, notification are all planned).
- `capabilities/default.json` grants `core:default` plus window minimize/maximize/unmaximize/toggle-maximize/is-maximized/close/start-dragging.
- The CSP in `tauri.conf.json` is stricter in spirit but longer than the sketch: `connect-src 'self' ipc: http://ipc.localhost ws://127.0.0.1:* http://127.0.0.1:* ws://localhost:*`, `script-src 'self' 'wasm-unsafe-eval'`, `img-src 'self' data: blob: asset: http://asset.localhost`, `font-src 'self' data:`, `worker-src 'self' blob:`, `media-src 'self' blob: data:`.

## Implementation notes (Phase 1 platform-shell)

- **MSRV raised to 1.90** (`[workspace.package] rust-version`): Tauri 2.12 and its dependency tree require it. Contract §1 updated. The bump enables newer clippy lints (`unnecessary_map_or`, `manual_is_multiple_of`); the two hits in `orkestr-events` were fixed.
- **Plugins:** `tauri-plugin-single-instance` 2.5 (registered first; a second launch shows, unminimizes and focuses the main window). `tauri` gains the `tray-icon` feature. Window-state, global-shortcut, updater and notification plugins are still not installed.
- **Tray:** icon = default window icon; menu *Show / Hide Orkestr*, *Stop All AI*, *Quit Orkestr*; left click toggles the window. *Stop All AI* emits `orkestr://tray/stop-all` to the webview, which calls `emergency.stopAll` over RPC (no business logic in Tauri); if the method does not exist yet the webview reports it and hides the window.
- **Close to tray:** `WindowEvent::CloseRequested` on `main` is prevented and the window hidden while `closeToTray` is on (default). The webview mirrors the core setting through the new command `shell_set_prefs({prefs:{closeToTray}})`; `shell_quit` quits even in tray mode. If the tray cannot be created, close-to-tray is disabled so the user is never stranded. (The ADR-0004 "Keep running / Stop / Cancel" dialog is not implemented; the setting replaces it.)
- **Autostart:** registered by the core (`settings.update {autostart:true}` → `PlatformAdapter::autostart()`), id `dev.orkestr.desktop`, `<exe> --autostart`. With `--autostart` the window stays hidden (`tauri.conf.json` now has `"visible": false`; `setup` shows it otherwise).
- **Commands (allowlist):** `core_connection`, `shell_set_prefs`, `shell_quit`. Capabilities add `core:window:allow-hide|show|set-focus|set-fullscreen|is-fullscreen`.
