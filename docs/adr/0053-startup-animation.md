# ADR-0053: Startup animation (Jarvis boot sequence)

Status: Accepted (wave 10, v1.3.0).

## Context
The user asked for a beautiful launch animation in the style of the Jarvis
reactor (ADR-0052). Orkestr's main window showed a plain splash
(`ConnectionScreen`) while the embedded core started. An intro must not make
the app slower to use, and must work before the core is connected (no RPC).

## Decision
* **Overlay over the splash** (`features/boot/`): `BootHost` in `App.tsx`
  mounts `BootOverlay` above everything while the core starts; the existing
  `ConnectionScreen` stays underneath (errors, retry, a skipped animation).
* **Sequencing** (`sequence.ts`, pure): the animation clock runs to a *gate*;
  it passes the gate only when the core connection is open (the full version
  also waits up to 600 ms for the other startup steps). If the core is slower,
  a «holding» idle loop plays (scan sweep, real step statuses). The rest
  (greeting, fly-to-title-bar) follows. It ends when both the animation's
  minimum and core readiness are met; click / Esc / any key skips to a 0.3 s
  exit; a failed start fades out to the connection screen's error.
* **Versions**: *full* (~4.85 s minimum) on the first start after install or
  update, *short* (~1.95 s) otherwise, *static* (fade, no motion) with reduced
  motion or in Performance / Battery Saver mode, none when switched off or
  when the window starts hidden (autostart to tray). Assistant mode and small
  windows get a compact layout.
* **First launch** = no `orkestr.boot.lastVersion` on this device or a
  different one (written once the core is ready); the core's `app-version.json`
  check (`updates_status().startup.start`) can upgrade short → full during the
  first 0.5 s.
* **Real progress**: once connected, read-only probes (`providers.list`,
  `nodes.list`, `plugins.list`) and the voice config load mark the boot log
  lines (`OK` / `ERR` / `—`). They never block (4 s timeout).
* **Settings are device preferences** in local storage (`orkestr.boot.prefs`):
  «Анимация запуска» (always / after install & updates / off, default always),
  «Звук запуска» (same, default after install & updates), «Голосовое
  приветствие» (default off). They live in Settings → Interface → «Запуск»
  with a preview button. The sequence needs them before the core connects, so
  they cannot be core settings; the environment it needs (sound settings,
  quiet hours, DND, performance mode, the Jarvis form of address) is mirrored
  to `orkestr.boot.env` while the app runs.
* **Sound** is synthesized with WebAudio (noise whoosh + sub swell + chime at
  the ignition) and follows the sound switch / volume, DND, quiet hours and
  Battery Saver; blocked autoplay is ignored silently.
* **Spoken greeting** goes through the existing TTS (`say`, needs spoken
  replies). Jarvis' own greeting of the day (ADR-0049) waits for the boot
  sequence and is not repeated when the boot spoke.
* **Rendering**: one full-window Canvas 2D (`renderer.ts`) in the visual
  language of the Jarvis reactor; glow sprites and the grid are cached; theme
  tokens via `readPalette`; DPR ≤ 2. HUD texts are DOM, typed out per frame.

## Consequences
* Startup looks deliberate and shows real progress; it never adds time once
  the core is ready later than the animation's gate, and adds at most the
  short version's ~1.1 s after the gate when the core is faster.
* Device-local settings do not roam between devices (like the perf auto-switch
  preferences).
* Debug hook (`localStorage["orkestr.debugHooks"] = "1"`):
  `__orkestrDebug.bootPlay(variant)` / `__orkestrDebug.boot.freeze(state)` for
  screenshots (`scripts/smoke/boot-animation-screenshots.mjs`).
