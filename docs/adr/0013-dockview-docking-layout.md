# ADR-0013: dockview for the docking layout

- **Status:** Accepted
- **Date:** 2026-10-02
- **Phase:** 1 (docking + presets), 3 (layout editor, detachable panels to OS windows, multi-monitor)
- **Spec:** §14, §28 (docking, detachable panels, floating windows), §29 (layout presets), §32, §34 (multi-monitor)
- **Related:** ADR-0003, ADR-0015 · **Spike:** SP-01

## Context

The Project Desktop (§14) has 15 or more panels (Manager, Code, Files, Terminal, Git, Agents, Tasks, Architecture, Browser, Database, Logs, Deployments, Memory, Costs, Monitoring). Users drag, resize, hide, dock and undock them, and save presets (Coding, Mission Control, Monitoring, Focus, Custom; §29). In Phase 3, panels can be detached into separate OS windows on other monitors (§34). The layout must be serializable so it can be part of the AI-editable UI config (ADR-0015).

## Decision drivers

1. Proven docking UX: tab groups, splits, floating groups, drag-and-drop between groups.
2. JSON serialization of the full layout.
3. React 18 support, zero runtime dependencies, theming through CSS variables (`--ok-*` tokens).
4. Popout/detach support we can map to Tauri windows.
5. Performance with Monaco and xterm inside panels (no remounts on drag).

## Considered options

| Option | Verdict |
|---|---|
| **dockview (+ dockview-react)** | **Chosen.** Docking, floating groups and popout windows built in; `toJSON`/`fromJSON`; zero-dependency core; CSS-variable theming |
| FlexLayout (caplin) | Good, but floating/popout support is weaker and its theming is class-based |
| rc-dock | Smaller community; serialization is OK |
| golden-layout v2 | Ageing React integration |
| Custom (react-resizable-panels + own DnD) | Rejected. Months of work to reach docking parity, with no product value |

## Decision

- **Panel registry:** `apps/desktop/src/panels/registry.ts` (integrator-owned, contract §8) maps a `PanelKind` string to a lazy component `({ projectId }: PanelProps) => JSX.Element` plus metadata `{ title, icon (lucide), minSize, singleton, phase }`. Panels for later phases are registered with an explicit "Available in Phase N" empty state, so the user knows the panel is not ready yet. These are not fake panels.
- **Layout model:** the dockview `SerializedDockview` JSON is stored as `UiConfig.layouts[<presetId>].dock` (ADR-0015). On load:
  1. the JSON is validated against the panel registry (unknown `PanelKind`s are dropped with a warning);
  2. it is then given to `fromJSON`.
  Built-in presets ship as JSON in `@orkestr/ui/presets` and can be reset.
- **Per project:** the layout is per *device* and per *project* (`layoutsByProject[projectId]`), falling back to the active preset (§34 "layouts per-device").
- **Heavy panels:** Monaco and xterm instances stay mounted while hidden (dockview `renderer: "always"` for those panels) so that dragging a panel does not lose editor state or the terminal scrollback.
- **Theming:** dockview's CSS variables are mapped to `--ok-*` tokens in `@orkestr/ui/dockview-theme.css`. No hardcoded colors.
- **Detach (Phase 3):** dockview's popout groups use `window.open`. In Tauri, the shell intercepts that and creates a `WebviewWindow` (frameless, custom title bar) through the `window_open_overlay`-style command `window_open_panel`. The panel is rendered there by route `/#/panel/<kind>?projectId=…`, connecting to the same core RPC. The window's monitor and bounds are stored in the layout (`popouts[]`), so multi-monitor layouts are restored (§34).
- **Keyboard:** `Ctrl+\`` toggles the terminal; the command palette (`Ctrl+K`) can focus or open any panel.

## Consequences

- **Positive:** full docking from day one; the layout is data, so AI-editable and versionable (ADR-0015); Phase 3 detach builds on a built-in feature.
- **Negative:** dockview's popout via `window.open` needs Tauri plumbing, and is validated in SP-01 (Phase 0) and SP-04 (Phase 3).
- **Negative:** a dependency on a single-maintainer library. Mitigation: the layout JSON is ours (wrapped), panel components do not import dockview, and only `apps/desktop/src/shell/Dock*.tsx` touches its API, so swapping it costs about one week.

## Implementation notes (Phase 0 foundation, 2026-10-02)

Implemented for the shell (`apps/desktop/src/components/Workspace.tsx`). **dockview 8 split its React bindings** into the `dockview-react` package; both `dockview` (framework-agnostic core) and `dockview-react` are installed. Theming uses a `DockviewTheme` object whose `className` must be a single class (`dockview-theme-nebula`), which re-declares the structural `--dv-*` variables. Panels come from `apps/desktop/src/panels/registry.ts` (15 panels) and the default layout is `CODING_PRESET`. The layout is persisted per webview in `localStorage` (dockview `toJSON`/`fromJSON`), not yet through the core (`ui.*`, ADR-0015); popouts are not implemented.
