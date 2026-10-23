# ADR-0045: One floating orb with an Agent Hub instead of per-agent robots

- **Status:** Accepted, implemented (Wave 8). **Supersedes** [ADR-0016](0016-floating-robots-transparent-windows.md).
- **Date:** 2026-10-23
- **Spec:** §30, §31, §34 (as revised by the owner's Wave 8 request #3)

## Context

ADR-0016 put one transparent always-on-top window on the desktop for every
visible agent (up to 6, plus an overflow cluster), each a procedural robot
with bubbles, a Robot Designer and drop-on-project assignment. In practice:

- several robots clutter the desktop and each window costs a renderer;
- the robots showed state but offered few actions (open, hide, stop);
- the owner asked for **one** small, modern, "Jarvis-style" sphere in the
  Orkestr palette and the selected theme that gives access to **all** agents:
  write to them, stop them and every other agent action.

## Decision

- **One window, `orb`.** Transparent, frameless, shadowless, always on top,
  skip-taskbar, not focused on open, on all workspaces. It loads the same
  bundle as every window (`index.html`; `main.tsx` routes by window label —
  the separate `robot.html` entry is gone). Window management lives in
  `apps/desktop/src-tauri/src/orb.rs` (replaces `robots.rs`):
  - main-window commands `orb_show {boxSize, positions}`, `orb_hide`,
    `orb_toggle_hub`, `orb_reset_position`;
  - orb-window commands `orb_set_shape`, `orb_set_layout`, `orb_to_main`
    (capability `capabilities/orb.json`, whitelisted requests).
- **Sphere:** 48–96 px (Settings offers 56 / 64 / 72; default 64) inside a
  box with 12 px of room for glow and badge. Rendered with Canvas 2D layered
  gradients (glow, drifting plasma blobs, specular highlight, rim), colours
  read from the theme tokens (`--ok-color-*`); the theme's `robot.accent`
  (schema name kept) overrides the orb accent (Theme editor → Orb tab).
- **Moods** (`features/orb/model.ts`), most urgent first: offline, listening
  (waveform ring from the mic level), speaking (ripples, pulsing core),
  stopped (emergency stop), approval (pulsing warning ring), error (danger
  tint; cleared once the hub has shown the failed items), working (1–3
  rotating rings), idle (breathing). Badge: approvals → errors → working.
- **Performance:** `orbMotion` → full / reduced / static. Static in Battery
  Saver, with "Still" chosen, when hidden, and while no Orkestr window has
  the focus (decorative loops pause) unless hovered, the hub is open or voice
  is live. Performance mode, `prefers-reduced-motion` and "Reduced" cap it at
  reduced (no particles / breathing, ≤ 15 fps). Full is capped at 30 fps.
- **Click-through:** the webview reports its shape (sphere circle + open
  flyout / menu box); a 30 Hz cursor poller (only while the orb exists)
  toggles `set_ignore_cursor_events`. The transparent corners of the sphere
  box never block clicks.
- **Drag / snap / position:** `startDragging()` after a 4 px move (only while
  the hub is closed); 400 ms without moves ends the drag; the sphere snaps
  to work-area edges and corners within 28 px and stays on screen. The
  settled position (physical px + monitor name) is remembered **per device
  and monitor** in local storage (`orkestr.orb.positions`, most recent
  first); on show the first entry whose monitor is connected wins, else the
  bottom-right corner of the primary monitor.
- **Agent Hub flyout:** click (or `Ctrl+Alt+O` from the main window, or
  Enter on the focused sphere) grows the window next to the sphere — right
  if it fits, else left, vertically centred and clamped to the work area;
  the sphere keeps its screen position (`hub_layout`, unit-tested). Contents:
  - Orkestr agents (`agents.list`) and live Claude Code sessions
    (`claudeCode.sessions` per project), grouped by project, attention first,
    with status, activity and turn progress; finished items stay 30 min;
  - per item: quick message (Claude sessions → `claudeCode.send`; Orkestr
    agents → the project's Manager via `manager.chat`, because the core has no
    per-agent inbox), Pause / Resume, Stop (`agents.stop`,
    `claudeCode.interrupt` / `stop`), Restart (spawn the same template on the
    same task), open transcript in the main window;
  - "Ask the Manager" per project;
  - pending approvals with inline Approve / Deny for low / medium risk
    without a danger category (same rule as the HUD); risky ones open the
    main window's inbox (Dangerous Action Guard unchanged);
  - two-step emergency stop (`emergency.stopAll`), "Open Orkestr", settings,
    push-to-talk (hold; the voice engine stays in the main window, signalled
    over a `BroadcastChannel`);
  - keyboard: ↑ / ↓ rows, Enter message box, Esc closes the box then the hub.
  Clicking elsewhere (window blur) closes it.
- **Orb menu** (right-click / Shift+F10): open hub, open Orkestr, animation
  level, badge, settings, hide.
- **Settings → Orb** (`UiConfig.prefs.orb`, additive protocol field
  `OrbPrefs {enabled, size, showBadge, animation}` — AI-editable and undoable
  like every pref): enable, size, badge, animation level, reset position,
  preview of the moods.
- **Migration:** the per-agent robot windows, Robot Designer, cluster window,
  drop-on-project, robot bubbles and their i18n / tests are removed. On first
  start a device whose robots were switched off (`robots.config.enabled =
  false`) gets the orb switched off too. The core keeps the `robots.*` RPC and
  table for compatibility (no longer used by the desktop); removal is a later
  protocol cleanup.

## Consequences

- One renderer instead of up to seven; no robot window churn on agent events.
- More actions from the desktop (message, pause / resume, restart, approvals,
  emergency stop, voice) at the cost of a larger flyout window while open.
- Messages to Orkestr agents are relayed by the Manager (documented in the
  hub placeholder); a direct agent inbox would need a new core API.
- The hub hotkey works while an Orkestr window has the focus (no global OS
  hotkey plugin).
- Not verified: macOS / Linux (same platform matrix caveats as ADR-0016).
