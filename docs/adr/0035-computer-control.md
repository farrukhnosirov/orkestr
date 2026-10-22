# ADR-0035 — Computer Control: the personal computer assistant

Status: accepted — implemented (Wave 8)

## Context

The owner asked for Orkestr to become a full personal computer assistant
controlled by voice and text, next to the project orchestrator: open, switch
and close programs, drive windows, keyboard and mouse, system functions,
files and the browser; a ready-made catalogue of 1000+ template commands;
user-created command sets in the spirit of LAITIS; an AI that proposes a plan
for unknown commands which the user approves or corrects and can then
remember as a new command; and two UI modes — *Orchestra* (the existing
workspace) and *Assistant* (a minimalist panel). Driving the real desktop is
the most dangerous capability Orkestr has, so it needs explicit safety rules.

§18 of the spec reserved this ADR ("Computer Control adapters") and the
permission engine already treats `Capability::ComputerControl` as never
granted by default to agents (P6). This ADR covers the *user's own* control
of the machine; agent use of the capability is unchanged (still denied /
always asked).

## Decision

### 1. Crate and boundaries

* New crate **`orkestr-computer`** (`crates/computer`): action vocabulary
  executor, matcher, catalogue, user sets, planner, safety, service.
* OS access is behind the synchronous trait **`ComputerBackend`**
  (`backend/mod.rs`), called on blocking threads:
  * `WindowsBackend` — `windows` 0.61 crate: `ShellExecuteW` (apps, URIs,
    `shell:AppsFolder\<AUMID>` for UWP / Store apps, `.lnk`, files,
    folders, `ms-settings:`), `SendInput` (keys, Unicode text, wheel,
    clicks, media / volume keys), `EnumWindows` + DWM cloaking +
    ApplicationFrameHost child resolution (window list with real process
    names), `SetForegroundWindow` with the Alt-key trick, `ShowWindow` /
    `WM_CLOSE`, UI Automation (`IUIAutomation` → click elements by
    accessible name via Invoke / Toggle / SelectionItem patterns or a real
    click; `CurrentIsPassword` of the focused element), `LockWorkStation`,
    `SetSuspendState`; PowerShell (no window, script on stdin, data as
    base64) for `Get-StartApps`, clipboard, toast notifications, WMI
    brightness, screenshots and the recycle bin; `sysinfo` for processes.
  * `UnsupportedBackend` (non-Windows): every call returns an explicit
    `Unsupported` error (RPC `unimplementedFeature` with details).
  * `MockBackend` records calls (tests; `ORKESTR_COMPUTER_BACKEND=mock`
    for screenshot runs and CI, `=unsupported` to switch control off).
* **Deviation from contract §2 / §9.4** ("all `#[cfg(target_os)]` code and
  FFI `unsafe` only in `orkestr-platform`"): the Windows backend lives in
  `crates/computer/src/backend/windows.rs`, the only module of the crate
  with `#![allow(unsafe_code)]` (the crate is `#![deny(unsafe_code)]`).
  Reason: the task assigned the backend to this crate and the code is large
  and specific to computer control; moving it into `platform` later is a
  mechanical change behind the same trait.

### 2. Action vocabulary (protocol)

`orkestr_protocol::methods::computer::ComputerAction` — 33 tagged actions:
`Launch, OpenUrl, WebSearch, OpenSettings, OpenPath, CloseApp{discard},
KillProcess, FocusApp, Window{op}, Keys{combo,times}, TypeText, Scroll{dir,
amount, unit Lines|Pages|Edge}, ClickElement, MouseClick, Media, Volume,
Brightness, System{Lock|Sleep|Shutdown|Restart|SignOut}, Screenshot,
ClipboardSet/Get, Notify, Timer, ListProcesses, ListWindows, FileCreate,
FolderCreate, FileMove, FileDelete, Wait, Speak, SetMode, If{condition,
then, otherwise}`. String fields accept `{slot}` templates; numeric fields
are `SlotNum = number | "{slot}"`. Client-side effects (speak, switch mode,
show a list, screenshot path) come back in the run report. The same
vocabulary is used by the catalogue, user sets, the command editor and the
AI planner.

### 3. Catalogue (1035 commands)

Generated systematically by `scripts/gen-assistant-catalogue.mjs` from data
tables (`crates/computer/data/{apps,settings,sites,folders}.json`: 85 apps,
76 Settings pages, 69 sites / 27 search engines, 17 known folders) ×
phrase templates (ru + en) plus hand-written key / window / scroll / media /
system / file commands and per-app context sets (browser, Office, File
Explorer, media players). Output `data/catalogue.json` (14 built-in sets,
1035 commands, 6561 phrases) is embedded with `include_str!`; a Rust test
checks ≥ 1000 unique ids, that every command round-trips, that slots used
by steps exist in every phrase, both languages and all categories.
`--check` verifies the committed file is current.

### 4. Matching (local first)

`text.rs` + `matcher.rs`: lowercase, `ё→е`, punctuation out, filler words
dropped (please / пожалуйста / wake words), a light Russian / English
stemmer, verb synonyms folded into concepts (`открой/запусти/open/launch →
@open`, …), number words ru/en (`двадцать пять`), durations (`5 минут`).
Phrases compile into token patterns; a small DP aligns them with the
utterance (missing words, extra words, fuzzy tokens by Levenshtein,
multi-word slots; typed slots `{n}`, `{duration}` are validated). Score
threshold 0.74; ranking prefers literal matches over slots, user sets
(+0.02) and active-window context sets (+0.03). ~6.5k patterns match in
milliseconds (debug build ≈ 28 ms incl. RPC).

### 5. User sets (LAITIS-like)

Sets are JSON files under `<data>/computer/sets/<id>.json`; built-in sets
can only be enabled / disabled (`state.json`). Import / export format
`{"format":"orkestr-command-set","version":1,"set":{…}}`; imports never
overwrite silently (`-2` suffix). A set (or a command) may carry an
`AppContext` (process names / title fragments) and is then active only while
that app is the user's window. The Settings → Assistant editor edits
phrases with slots, multi-step actions with per-type fields, `If`
conditions (JSON), safety level and context, and can test-run commands.

### 6. Unknown commands → AI plan

`computer.handle` with `planUnknown` (Assistant mode) asks the provider
stack through `PlanModel` (core: the AI Manager's model, then routing;
the offline Mock answers prompts carrying the marker "Orkestr's computer
assistant planner" deterministically). The model must answer
`{explanation, steps}` with the same vocabulary; plans are validated
(known actions, depth ≤ 4, ≤ 64 steps, no slots) and **never run before
approval**. `computer.plan{planId, correction}` re-plans with the user's
correction; `computer.approvePlan` runs it (risky steps still ask);
"запомни это как …" / "remember this as …" (or the button) saves it as a
command in the `my-commands` set (`computer.rememberPlan`).

### 7. Safety

* Static classification (`safety.rs`): close without saving → Confirm;
  kill process, delete, shutdown / restart / sign out → Dangerous; move,
  sleep → Confirm. Protected processes (explorer, csrss, lsass, svchost,
  Orkestr itself, …) and protected paths (drive roots, profile roots, and
  everything the **Dangerous Action Guard** hard-denies for the equivalent
  `Remove-Item -Recurse` command) are **refused**.
* Runtime checks: typing into a focused **password field** (UIA) and closing
  a window whose title looks unsaved ask first.
* Every confirmation is a real **approval** (`ApprovalService`,
  `Capability::ComputerControl`, preview of the steps, 10-minute TTL): it
  shows up in the approvals inbox and can be resolved there, by
  `computer.confirm`, or by voice ("да" / "нет"); the run executes when the
  approval is approved — from wherever.
* Input never goes to Orkestr's own windows: the target is the user's
  window (foreground unless it is Orkestr, else the next in z-order);
  `Launch` waits for and focuses the new window before following steps.
* Tabbed editors that persist sessions (Windows 11 Notepad) treat "close
  without saving" as *close the active tab, answer Don't save* — otherwise
  the unsaved text would survive in the session; they are never killed.
* **Audit**: every run, denial, confirmation request and rejection writes a
  `computer.run` audit row (step types + targets, never typed text or
  clipboard content). `ComputerCommand` events (append-only kind) carry ids,
  titles and outcomes only.
* **Global stop**: `computer.stop` and `emergency.stopAll` cancel running
  pipelines (also inside waits / backend calls), pending confirmations
  (approvals cancelled) and timers.
* **Remote never**: the whole `computer.*` area is `MethodPolicy::local()`
  (admin + step-up + local-only — remote principals are refused before the
  handler), including `computer.stop`; a policy test pins it.

### 8. UI modes and voice

* Mode switch in the title bar, `Ctrl+Alt+A` (palette command), and by voice
  (catalogue `mode.*`: "режим ассистента" / "assistant mode", "режим
  оркестра"). Assistant mode makes the main window a compact (460×680),
  centred, always-on-top panel and restores the previous geometry on the
  way back; the workspace stays mounted but hidden, the title bar is
  reduced to brand, mode switch, stop and window controls.
* The panel: voice orb (hold to talk) with state, last transcript, reply,
  confirmation / plan cards, quick command search with live matches,
  examples, recent commands, stop, command sets, back to Orchestra.
* Voice integration is a single hook in the existing pipeline
  (`features/voice/controller.ts` → `features/assistant/voiceRoute.ts`): STT /
  TTS engines and wake word stay in the voice feature (so switchable engines
  compose). Orchestra mode: Orkestr intents win, utterances the orchestrator
  did not resolve go to the local matcher (no AI plan; the Manager still gets
  the rest). Assistant mode: strong computer matches win, Orkestr intents
  still work, everything else gets an AI plan; yes / no / "исправь …" answer
  the assistant's prompts.

## Consequences

* Real desktop control is verified on Windows 11 by
  `scripts/smoke/assistant-acceptance.mjs` (47 checks: Notepad open / type /
  copy / scroll, window switching, close without saving through the
  approvals inbox, `ms-settings:` page open + close, AI plan → correction →
  approve → remember → rerun, safety) and by Rust / vitest suites with the
  recording backend.
* Brightness works only on panels with WMI brightness (laptops); global
  hotkeys outside the app (system-wide `Ctrl+Alt+A`) need the Tauri
  global-shortcut plugin (not installed) — the in-app shortcut, title bar and
  voice work.
* macOS / Linux backends are not implemented (`Unsupported`).
