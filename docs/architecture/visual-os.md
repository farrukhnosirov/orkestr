# Visual OS (Phase 3)

How the Phase 3 "Visual OS" layer is put together (§28–§34): themes, layouts
and their persistence, floating robot windows, the voice privacy model,
performance modes and the safety rules for the AI-editable UI.

Decisions live in the ADRs. This page describes the system as built at Phase 3
acceptance (2026-10-05):

| Topic | ADR |
|---|---|
| Docking and detached windows | ADR-0013 |
| UI config schema | ADR-0015, schema v2 |
| Floating robots | ADR-0016 |
| Local wake word | ADR-0017 |

The UI never talks to Tauri for business data (§67). Everything below that is
*data* goes through the RPC protocol (`ui.*`, `robots.*`, `voice.*`). Tauri
commands only manage windows: HUD, robot windows and detached panels.

## 1. One UI config document

`orkestr-protocol::ui_config::UiConfig` is the single source of the
appearance. It is stored by the core with a version history. There is a
global document and an optional per-device override (`ui.getConfig` /
`ui.setConfig` with `deviceId`).

```
UiConfig (schemaVersion 2; v1 documents are upgraded on read)
├─ theme            ThemeConfig
│  ├─ id, name, mode (Dark | Light), description, author
│  ├─ tokens        map token → CSS value (≤ 256, value ≤ 512 chars)
│  ├─ effects       blurPx, glowIntensity, panelOpacity, radius, spacing, shadows, animations
│  ├─ background    kind (Solid | Gradient | Nebula | Grid | Particles) + params
│  ├─ motion        durations / easing scale
│  ├─ sounds        preset + per-event cues (sound engine ids)
│  └─ robot         default skin / accent for robots the user has not styled
├─ layout           LayoutConfig: active preset id
├─ performanceMode  UltraVisual | Balanced | Performance | BatterySaver
└─ prefs?           density, panel placement, hidden widgets, background dim, HUD look
```

**Validation runs on both sides with the same rules.** Rust is the
authority. The TypeScript mirror exists for editor UX.

Token values are restricted to colors, lengths, gradients and
`var(--ok-…)` references. These are rejected in both mirrors:

- `url(`, `expression(`, `javascript:`, `data:`, `@import`, `attr(`, `image-set(`, `eval(`
- the characters `;` `{` `}` `<` `>` `\` and backtick

As a result, a theme cannot load a remote resource or inject CSS.

**Built-in themes:**

| Theme | Notes |
|---|---|
| Nebula | Default; WebGL nebula background |
| Midnight Pro | |
| Aurora | |
| Carbon | Perspective grid background |
| Solar Light | Light theme |
| High Contrast | AAA |

Each built-in theme carries a WCAG contrast report (`ui_theme::contrast_report`). The Theme Editor shows that report live and does not block saving.

**Theme engine** (`features/theme/ThemeRuntime.tsx`, `@orkestr/ui`):
- It turns the active document into `--ok-*` CSS variables on `<html>`, plus the background layer.
- Every window runs it: main, HUD, robots and detached panels. It re-reads on `UiConfigChanged`, so live edits reach every window.
- Monaco and xterm themes are derived from the same tokens.

## 2. Layouts and their persistence

The layout presets are:

| Preset | Shortcut |
|---|---|
| Coding | Ctrl+1 |
| Mission Control | Ctrl+2 |
| Monitoring | Ctrl+3 |
| Focus | Ctrl+4 |
| Review | Ctrl+5 |
| Custom | — |

Users can also save their own named layouts.

**Docking:** in-window docking uses dockview. A panel can float inside the window, or be detached into its own frameless OS window (`#/panel/<id>`) with a re-dock button.

**Persistence is per device** (`ui.layouts*`, `methods/layouts.rs`). A
`LayoutDocument` holds:
- the serialized dock JSON (≤ 256 KiB; opaque to the core, validated against the panel registry by the UI);
- the visible panels (≤ 64);
- the lock flag;
- the window arrangement: detached windows with their monitor and rect (≤ 16);
- optional Mission Control widget arrangement (`widgets`, ≤ 16 entries × 16 KiB).

Each device stores up to 64 layouts. Layouts never leak between machines, because each one has its own monitors (§34 "layouts per-device"). When a stored monitor is missing, its detached windows fall back to the primary monitor.

**Layout Editor:** a side sheet with:
- lock;
- presets;
- save as, reset, export and import;
- a per-panel list with placement (main window or a monitor), float, detach and hide.

## 3. Floating robot windows

Each robot is bound to a real agent id (§30). It is its own small
`WebviewWindow` with these properties:
- labelled `robot-<agentId>`;
- loads `robot.html`;
- transparent, frameless, shadowless;
- always on top;
- hidden from the taskbar;
- not focusable on open.

One more window, `robot-cluster`, collects the overflow robots (Settings → Robots → "Robots on the desktop").

- **Who decides visibility:** the main window's `RobotHost` watches agent events over RPC. It opens and closes windows through `robot_open` and `robot_close`. Rust only manages windows; no business data passes through it.
- **States:** the robot state is derived from the agent state: Idle, Listening, Thinking, Coding, Testing, Reviewing, Waiting, Error, Completed. Pipeline events of the agent's task also switch it to Testing or Reviewing. Each state has a pose and an animation. Important events add a speech bubble, such as "Done! ✓", "Waiting for you", "Tests failed" or "Review approved ✓". A finished robot stays for a configurable time (2 min by default) and then leaves.
- **Click-through:**
  - Windows start with `set_ignore_cursor_events(true)`, which sets `WS_EX_LAYERED | WS_EX_TRANSPARENT` on Windows.
  - While robots exist, a 30 Hz poller hit-tests the global cursor against the robot's reported shape (ellipses and rectangles).
  - Cursor events are enabled only while the cursor is over the robot. An idle (pass-through) robot needs a 600 ms hover first.
  - Clicks anywhere else go to the app underneath.
- **Interaction:**
  - Click opens the agent in the Agents panel.
  - Right-click opens the context menu: open, assign to project, voice, hide, stop agent.
  - Drag snaps to work-area edges and other robots.
  - Dropping a robot on a project tile in the main window assigns the agent there.
- **Look:** Robot Designer (Settings → Robots) offers:
  - 2D (SVG) or 3D (three.js);
  - skin, material, accent, size, accessories, animation intensity;
  - voice, sound set, behaviour (snap, auto-hide idle, click-through idle, bubble level).

  Designs are stored per template through `robots.setConfig`. A theme's robot skin and accent apply to robots the user has not styled.
- **Voice:** a robot speaks with the agent's voice profile, with the robot design's voice id on top. A robot with voice on speaks its important bubbles, and the general voice notifications skip that agent so nothing is announced twice.

## 4. Voice privacy model

Voice follows §33, ADR-0017 and `features/voice/controller.ts`.

**When the microphone opens.** Only for:
1. Local wake-word spotting. MFCC + DTW templates are enrolled on this device and live only in this webview; the audio never leaves it.
2. One command after push-to-talk (Ctrl+Space, hold) or after the wake word.
3. Enrollment and the level meter in Settings.

The title-bar mic indicator names every open purpose. The HUD voice indicator mirrors the main window's mic state.

**Speech-to-text engines:**

| Engine | Where audio goes |
|---|---|
| Disabled (default) | The HUD takes typed commands |
| Browser speech (WebSpeech) | The OS / browser service |
| OpenAI Whisper | The cloud |

Any engine that sends audio off the device needs explicit, recorded consent first (`voice.setConsent`). The core enforces it on `voice.transcribe` and `voice.synthesize`. No constant microphone stream is ever sent to the cloud.

**After recognition:**
- Audio buffers are dropped right after recognition.
- Only the transcript, the intent id and the outcome are recorded (`voice.recordCommand` → `VoiceCommand` event).

**Intents:**
- The intent parser runs locally and is pure. It handles Russian and English, the wake phrase, and fuzzy project and agent names, for example "Скулифай" → Schoolify.
- Destructive intents need a spoken or clicked confirmation before they run: stop a project, stop all, approve or reject.

**Replies:**
- Text-to-speech uses system voices (`speechSynthesis`, offline) by default.
- OpenAI TTS is opt-in, with consent; only text is sent.
- Voice notifications respect Do Not Disturb and quiet hours.

**Acceptance seam.** `simulateTranscript(text)` runs a transcript through exactly the post-STT path, with no audio. It is exposed as `window.__orkestrDebug.voice` only when the local-storage flag `orkestr.debugHooks` is `"1"`. `scripts/smoke/phase3-acceptance.mjs` uses it for §63 G.

## 5. Performance modes

`features/performance/policy.ts` holds one table, so that every visual
feature degrades together:

| Mode | Blur/glow | Motion | Background animation | Robots | Charts | GPU effects |
|---|---|---|---|---|---|---|
| Ultra Visual | strong | full | yes | 60 fps, 3D allowed | 1 s | yes |
| Balanced (default) | normal | full | subtle | 30 fps, 2D | 2 s | no |
| Performance | off | reduced | no | 15 fps, 2D | 5 s | no |
| Battery Saver | off | off | no | still pose | 10 s | no |

The effective policy is written to `<html>` as `data-ok-*` attributes and CSS
variables. Robots, Mission Control charts, voice visuals, the HUD and theme
backgrounds read it.

Overrides on top of the chosen mode:
- `prefers-reduced-motion` caps motion at "reduced".
- A hidden or minimized window pauses everything, including theme backgrounds.

Automatic switching:
- Battery Saver turns on when the device runs on battery (`nodes.power`, opt-in).
- Sustained frame-budget misses step the mode down once, never below Performance.

The wake-word listener pauses in Battery Saver when the user allows it.

**Known gap:** robot windows read the *chosen* mode from settings. They do not get the main window's automatic downgrade from battery or frame rate.

## 6. AI-editable UI: safety

The Manager changes the UI **only** through validated RFC 6902 patches
(`ui.previewPatch` / `ui.applyPatch`, `orkestr-protocol::ui_patch`). It never
writes code. A patch is accepted only if **all** of the following hold:

1. **Size:** ≤ 64 operations (`add`, `remove`, `replace`, `test`); each value ≤ 16 KiB of JSON.
2. **Allowlisted paths.** AI actors may touch only:
   - `theme.tokens.*`, `theme.effects[.*]`, `theme.mode`, `theme.name`;
   - `layout.preset`;
   - `performanceMode`;
   - `prefs.*`.

   AI actors can never touch `schemaVersion`, the document root or the raw docking JSON.
3. **Safe strings:** no markup or code-like construct in any string (the same filter as tokens).
4. **Schema:** the result deserializes into the typed `UiConfig` with no unknown members.
5. **Validation:** `UiConfig::validate` passes (ranges, token grammar, ids).

**Flow:**
1. Natural-language request to the Manager. Example: "Make Mission Control more compact, put Claude on the right, hide the token chart and make the background darker".
2. The Manager uses its `update_ui_config` tool.
3. `ui.previewPatch` returns a human-readable change list and the inverse patch.
4. A card shows the changes, with a live preview overlay. Nothing is stored yet.
5. The user picks **Keep** or **Revert**. Keep runs `ui.applyPatch` and stores a new version, applied under the same write lock as the editors.
6. **Undo** applies the inverse patch as a new version.

The history is visible in Settings → Interface.

Applied changes are drawn by the theme engine. The AI-UI layer only overlays a proposal while it is being previewed. A patch that changes `layout.preset` switches the layout. A patch can hide Mission Control widgets by id. Emergency Stop and the approvals indicator live in the title bar, which no patch can hide.

**Known gaps:**
- Contrast is reported, but a patch that lowers contrast below AA is not rejected.
- There is no `--safe-ui` recovery flag yet.
- AI patches change only the global document, not a device override.

## 7. Verification

`scripts/smoke/phase3-acceptance.mjs` drives the real app. It uses a debug build with the bundled frontend, a temporary `ORKESTR_HOME`, the mock provider and a temp git project named "Schoolify". It covers:
- Scenario H with robot transparency and click-through checks;
- Scenario G through the voice seam with a TTS spy;
- every theme, layout preset and performance mode;
- the HUD;
- AI patch → preview → keep → undo;
- detaching and re-docking a panel.

It also checks that each window mounts exactly one root and that the page never scrolls the shell.

Screenshots are webview-only captures through DevTools, never the screen. Robot windows are captured with a transparent background and composited over a synthetic desktop. The screenshots are in `docs/screenshots/phase3/`, with `acceptance.json`.
