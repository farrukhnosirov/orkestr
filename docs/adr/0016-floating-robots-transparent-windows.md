# ADR-0016: Floating robots as separate transparent Tauri windows

- **Status:** Superseded by [ADR-0045](0045-floating-orb-agent-hub.md) (2026-10-23: one floating orb with an Agent Hub replaces the per-agent robot windows; the robot windows, Robot Designer and `robots.rs` were removed). Originally: Accepted (implementation conditional on SP-04).
- **Date:** 2026-10-02
- **Phase:** 3 (Phase 0 spike SP-04 de-risks it early)
- **Spec:** §30, §31, §34, §61 (floating transparent window spike), §62.1, §63H
- **Review:** RR-C4, RR-A12
- **Spike:** SP-04 · **Risk:** R-01

## Context

Each floating robot represents a real `AgentId` (§30). It lives on the OS desktop outside the main window: transparent, frameless, always on top, click-through when idle, draggable, snapping, with a speech bubble, context menu, task status and voice. Dropping a robot on a project assigns the agent. §62.1 names "Tauri floating/transparent windows on three OSes" as the top technical risk.

## Decision drivers

1. Per-robot OS-level positioning across monitors (§34).
2. Pixel-accurate click-through: transparent areas pass clicks, the robot body does not (RR-C4).
3. A low GPU, CPU and RAM budget (§31): robots must not slow AI work.
4. Cross-platform degradation (Wayland, X11 without compositor).

## Considered options

| Option | Verdict |
|---|---|
| **A. One small transparent `WebviewWindow` per visible robot** (~220×260 px plus the bubble area) | **Chosen** |
| B. One fullscreen transparent overlay window per monitor that hosts all robots | Rejected as primary. A fullscreen always-on-top window must be click-through except over robots, so cursor hit-testing runs over the whole screen. It interferes with fullscreen apps and games, blocks screenshots and screen sharing, and DPI handling across monitors gets complicated. **Kept as the fallback** if SP-04 measures per-window memory above budget |
| C. Native windows (Win32 layered windows / NSPanel) rendering Rive natively | Rejected. Three native renderers break §47 |
| D. Robots only inside the main window | Rejected as primary (it fails §30), **kept** as the "docked robots" fallback for Wayland and for no-compositor Linux |

## Decision

- **Window spec (created from Rust through the Tauri command `window_open_overlay`):**
  ```rust
  WebviewWindowBuilder::new(app, format!("robot-{agent_id}"), WebviewUrl::App(format!("index.html#/robot/{agent_id}").into()))
      .transparent(true).decorations(false).shadow(false).always_on_top(true)
      .skip_taskbar(true).resizable(false).focused(false).visible_on_all_workspaces(true)
      .inner_size(220.0, 260.0).position(x, y)
  ```
  - macOS needs `macOSPrivateApi: true` for transparent webviews. This is acceptable for a non-App-Store build (§47) and is re-evaluated if a Mac App Store build is ever needed.
  - Windows: WebView2 transparency needs a `transparent` body background, and acrylic is not used for robots.
- **Click-through (RR-C4):**
  - A Rust task in `src-tauri::overlay` polls the global cursor position at 30 Hz, and only while robots are visible.
  - Per robot, it tests the cursor against the robot's hit shape: an ellipse plus the bubble rectangle, reported by the robot webview whenever its animation state changes.
  - It calls `set_ignore_cursor_events(!inside)` only when the result flips.
  - In *pass-through* mode (agent `Idle`), `inside` also requires the modifier key (Alt) or a 600 ms hover.
- **Behaviour:**
  - Dragging uses `startDragging()` from the robot body.
  - Snap: on drag end, position snaps to screen edges or corners and to the other robots (16 px threshold), computed from monitor work areas (`available_monitors`).
  - Positions are stored per monitor id in `UiConfig.robots.positions`.
  - Dropping a robot onto the main window's project tab is detected with a cursor hit-test against main-window drop zones, reported to the overlay task. This avoids OS drag-and-drop across webviews and calls `agents.assign {agentId, projectId}`.
- **Rendering:**
  - Rive (`@rive-app/react-canvas`) is the default for robots: state machine inputs map one-to-one from the agent state (RR-A12).
  - Lottie is used for simple skins.
  - Three.js is opt-in for "Ultra Visual".
  - Frame rate is capped per Performance Mode (Ultra 60, Balanced 30, Performance 20, Battery 10 / static), and rendering pauses when the robot is fully idle and not hovered.
- **Data:** each robot webview connects to the same core RPC (`core_connection`) and subscribes only to `agent:<id>`. It does not load the main bundle; it uses the separate entry `robot.html` (code-split, target ≤ 300 KB JS gzip excluding the Rive runtime).
- **Budget:**
  - Default maximum is **6 visible robots**. Further ones collapse into a "robot dock" stack.
  - SP-04 budget per robot window on Windows: ≤ 40 MB extra private working set, ≤ 1% CPU idle, ≤ 3% CPU animating at 30 fps on the reference machine. **If this is exceeded, switch to option B (per-monitor overlay)** with a superseding ADR.
- **Platform matrix:**
  | Platform | Mode |
  |---|---|
  | Windows 10/11 | Full |
  | macOS | Full (transparent via private API; always-on-top across Spaces via `visible_on_all_workspaces`) |
  | Linux X11 + compositor | Full |
  | Linux X11 without compositor | Docked fallback (no transparency) |
  | Linux Wayland | Docked fallback by default. Experimental floating mode (no global positioning or always-on-top guarantee; layer-shell is not available to webviews) |
- **Accessibility and safety:**
  - Robots never steal focus (`focused(false)`) and are hidden while a fullscreen app is focused (optional, Windows `SHQueryUserNotificationState`, through `PlatformAdapter`).
  - The context menu always offers **Stop agent**.

## Consequences

- **Positive:** native-feeling desktop robots, per-monitor placement, and agent identity shown on the desktop.
- **Negative:** per-window webview overhead (WebView2 shares one browser process, but each window adds a renderer). This is bounded by the max-visible limit and validated by SP-04.
- **Negative:** cursor polling costs a little CPU. It is on only while robots are visible, at 30 Hz, and SP-04 measures it.
- **Negative:** Linux Wayland support is degraded, and this is documented.

## Implementation notes (Phase 0 foundation, 2026-10-02)

Not implemented yet. The main window already uses `transparent: true` with WebView2 (the app paints its own opaque background); floating, always-on-top, click-through windows and their Tauri commands do not exist. SP-04 has not run; macOS/Linux behaviour is unverified (`../spikes/foundation-tauri-websocket.md`, finding 7).

## Implementation notes (Phase 3, robots area)

- **Shell:** `apps/desktop/src-tauri/src/robots.rs` (commands `robot_open`, `robot_close`, `robot_close_all`, `robot_set_shape`, `robot_set_drop_zones`, `robot_to_main`; capability `capabilities/robots.json` for `robot-*`). Windows are `robot-<agentId>` plus `robot-cluster`, loading the separate entry `robot.html` (`#/agent/<id>`, `#/cluster`, `#/preview`). The name `window_open_overlay` from the decision above was not used.
- **Click-through:** a 30 Hz cursor poller (running only while robots are open) hit-tests the ellipse plus the bubble/menu rectangles that the robot webview reports. Pass-through (idle robots, `behaviour.clickThroughIdle`) needs a 600 ms hover. The Alt-modifier variant is not implemented.
- **Drag / snap / drop:** `startDragging()` starts after a 4 px move, and a drag ends after 400 ms without moves. The cursor is then hit-tested against ProjectRail tiles (`data-robot-drop`, reported by the main window). A hit sends `orkestr://robot/drop`, which opens the assign dialog. Otherwise the robot snaps (16 px) to work-area edges and corners and to other robots. There is no `agents.assign` RPC yet, so assigning spawns the robot's template in the target project (`features/robots/model/assign.ts`).
- **Data:** `robots.list`, `robots.config` and `robots.setConfig` use the core table `robots_config`, which holds one validated document: on/off, `maxVisible` (default 6), `lingerSeconds`, the default design and per-template designs, and positions per template id (physical px, with the monitor name). Positions live there instead of `UiConfig.robots`.
- **Rendering:** a procedural SVG robot (2D) and a lazy three.js low-poly robot (3D, Ultra Visual only), both drawing the same pose function. There are no Rive or Lottie assets. FPS tiers are Ultra 60, Balanced 30, Performance 15 and Battery Saver static. Idle robots rest unless hovered, and `prefers-reduced-motion` makes robots static.
- **Not verified:** macOS and Linux, the SP-04 memory/CPU budget measurements, and hiding robots while a fullscreen app is focused.
