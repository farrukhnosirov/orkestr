// Shared by `build.rs` (`include!`) and the crate's tests: every command in
// `generate_handler!` (lib.rs) must be listed here. `tauri_build` then
// generates `allow-<command>` / `deny-<command>` permissions, and a command is
// callable only from windows whose capability grants it (capabilities/*.json).
// Without this list Tauri allows every app command from every window.

/// App (shell) commands, in `generate_handler!` order.
pub const APP_COMMANDS: &[&str] = &[
    "core_connection",
    "shell_set_prefs",
    "shell_quit",
    "set_ui_language",
    "window_list_monitors",
    "window_open_panel",
    "window_close_panel",
    "window_redock_panel",
    "window_list_panels",
    "window_main_placement",
    "window_move_to_monitor",
    "window_reconcile",
    "hud_show",
    "hud_hide",
    "hud_toggle",
    "hud_collapse",
    "hud_expand",
    "robot_open",
    "robot_close",
    "robot_close_all",
    "robot_set_shape",
    "robot_set_drop_zones",
    "robot_to_main",
    "vscode_window_open",
    "vscode_window_close",
    "vscode_window_reload",
    "vscode_window_is_open",
    "vscode_open_external",
    "widget_document",
];
