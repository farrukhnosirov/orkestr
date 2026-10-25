# ADR-0050 — Cursor capture mode (AI computer use)

Status: accepted — implemented (wave 9, request 4)

## Context

The owner asked: when activated by voice («Джарвис, возьми управление»,
«захвати курсор») or from the UI, the personal AI inside Orkestr takes the
mouse and keyboard, performs a task described in natural language
(«открой настройки блютуз и включи его», «найди в браузере билеты в Ташкент
на завтра», «закрой все окна кроме VS Code») and hands control back.

The computer assistant (ADR-0035) already has a command vocabulary, a
Windows backend (`SendInput`, UI Automation, ShellExecute) and safety
confirmations, but every command is a fixed or AI-planned *blind* script.
Open-ended tasks need a closed loop that looks at the screen after every
action. Driving the user's real desktop is the most dangerous thing Orkestr
does, so the safety design is the core of the decision.

Later in the wave the owner added: a clear switch «Источник модели: Мои
подписки (Claude Code / ChatGPT) ↔ API-ключи», user-ordered backends inside
each source, and immediate failover to the next backend when one stops
working (auth, usage limit, rate limit, network/5xx, model unavailable).

## Decision

### Agent loop (`orkestr-computer::cursor`)

One session at a time runs **observe → decide → check → act → verify** until
the model says `done`, the user stops it, or a limit is hit:

1. **Observe** — the monitor showing the foreground window (or the cursor's
   monitor) is captured with GDI `BitBlt(CAPTUREBLT)` and box-downscaled so
   the longer side is ≤ 1280 px (setting 640–1600), PNG-encoded. The
   foreground window's UI Automation tree (control view, one
   `FindAllBuildCache` with name / control type / bounds / enabled /
   offscreen / password / toggle state / value) yields ≤ 120 elements,
   interactive ones first, on-screen only, mapped to screenshot pixels and
   numbered. Plus cursor position and window title / process. Orkestr's own
   windows contribute no elements.
2. **Decide** — a `ComputerUseModel` returns one `CursorAction` with a short
   thought, the remaining plan, an optional spoken phrase and a self-declared
   risk. The prompt asks for **one JSON object**; the parser tolerates fences,
   prose, common aliases (`double_click`, `key`, …) and repairs once.
3. **Check** — `guardrails::check` (below).
4. **Act** — absolute, DPI-correct `SendInput` (`MOUSEEVENTF_ABSOLUTE |
   VIRTUALDESK`, every event tagged with a `dwExtraInfo` marker); typing in
   12-character chunks with a stop check and a "foreground is not Orkestr"
   re-check between chunks; keys through the assistant's key names; `launch`
   and `focus_window` reuse the assistant executor (`Launch`, `FocusApp`).
5. **Verify** — the next observation; failures are reported to the model as
   notes ("element #7 does not exist", "the user rejected that action").

Action vocabulary: `click` (left / right / middle, double), `move`, `drag`,
`scroll`, `type`, `keys`, `wait`, `focus_window`, `launch`, `ask_user`,
`done {summary, success}`. Targets are an **element id** (preferred — the
prompt says so) or `x, y` in screenshot pixels mapped back with the
monitor's origin and scale (multi-monitor, negative origins, per-monitor
DPI v2 threads). Three consecutive errors, a repeated unusable answer or an
exhausted backend chain fail the session.

### Model backends (`ComputerUseModel`, `select`)

`ComputerUseModel::decide(request) -> Decision` is the seam. Most backends
are a `VisionCompleter` ("system + text + one PNG → text") wrapped by
`JsonVisionModel`, which owns the prompt and parsing. Core
(`crates/core/src/cursor_models.rs`) provides:

| id | source | transport | verified on this machine |
|---|---|---|---|
| `claudeCode` | subscriptions | Claude Code CLI `-p --input-format stream-json --output-format stream-json --tools "" --no-session-persistence --setting-sources "" --strict-mcp-config --system-prompt …`, the screenshot as a base64 `image` content block | **yes** — full real session on the smoke window: 3 actions, 3.5–4.6 s per step (Claude subscription, `sonnet`) |
| `codex` | subscriptions | Codex CLI `exec --json -s read-only --skip-git-repo-check --ephemeral -i <png> -` (ChatGPT sign-in) | transport yes (image → correct JSON, ~14 s per call); no full session run |
| `anthropic`, `openai`, `gemini` | API keys | the provider stack (`ChatMessage::images`) with the vault key of an enabled provider of that kind | not run (no Anthropic key; OpenAI / Gemini keys exist but were not spent) |
| `mock` | — | `ScriptedModel` (tests; `ORKESTR_CURSOR_MOCK_SCRIPT=<json actions>` or `ORKESTR_COMPUTER_BACKEND=mock`) | unit / integration tests |

So **the user's existing subscriptions drive cursor mode without any API
key**: Claude Code works end to end, Codex accepts images too.

We deliberately use *our* JSON action protocol for every backend instead of
the vendors' native computer-use tools (Anthropic `computer_2025…`, OpenAI
`computer-use-preview`): those are coordinate-only, beta, tied to specific
models and API keys, and cannot use UI Automation element ids, which are far
more reliable than pixel guesses. A native-tool backend can be added behind
the same trait if it ever pays off.

**Selection + failover** live in one module (`cursor::select`) so the
integrator can swap them for the central provider chain (package D):
settings carry `source` (`subscriptions` | `api`), `subscriptionOrder`,
`apiOrder` and per-backend model overrides. A session gets a
`FailoverModel` over the *available, consented* backends of the selected
source in the user's order. `ModelErrorKind::classify` maps CLI / HTTP error
text to `Auth`, `Limit`, `RateLimit`, `Network`, `Unavailable` (fail over,
sticky for the session, `ComputerCursor{event:"failover"}` +
«Переключаюсь на …»), `BadOutput` / `Other` (no failover). Failover never
crosses into the other source (subscriptions never silently start billing
an API key).

### Safety

* **Explicit start only** — voice trigger, the Assistant-mode panel, the orb
  hub button, the palette (`cursor.start`, Ctrl+Alt+J). Never from a plan,
  an agent or a remote principal: every `computer.cursor*` method is in the
  local-only `computer` area (Admin + step-up, policy test).
* **Visible state** — a click-through (`WS_EX_LAYERED | WS_EX_TRANSPARENT |
  WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW`, colour-keyed) top-most overlay draws a
  4 px accent border on every monitor and a floating Jarvis badge «ИИ
  управляет курсором — нажмите Esc или пошевелите мышью, чтобы остановить» +
  the current step. It is excluded from every capture with
  `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` (verified: the border
  never appears in the screenshots the model gets).
* **Instant abort** — a dedicated thread with a message loop owns
  `WH_KEYBOARD_LL` + `WH_MOUSE_LL` hooks (installed before the first action;
  if they cannot be installed the session does not start). A *physical* Esc
  (`!LLKHF_INJECTED`) or physical mouse movement > 6 px / click / wheel
  (`!LLMHF_INJECTED`; our own injected moves reset the anchor) cancels the
  session token at once — mid model call, mid typing. Measured: Esc → aborted
  in 62 ms, mouse → 94 ms. Injected Esc (e.g. the model pressing Esc) does not
  abort (tested). Ctrl+Alt+Del / Win+L switch to the secure desktop: capture
  fails and three blind observations abort (`secureDesktop`). Stop in the
  panel, `computer.stop`, the emergency stop and the step / time limits
  (default 30 steps / 5 min) end it too.
* **Hand-back points** — pause, a confirmation and a model question disarm
  the hooks, restore Orkestr's windows and show "waiting" on the badge; the
  user has the mouse. Resume re-arms.
* **Never Orkestr** — Orkestr's windows are excluded from capture and (by
  default) minimized during the session, restored after; any pointer target
  inside an Orkestr window, typing / keys while one is in front, launching or
  focusing "Orkestr" are refused (`ownWindow`).
* **Never secrets** — typing text that the redactor recognizes (vault
  values, key patterns), card numbers (Luhn) or long high-entropy tokens is
  refused (`secret`); the model is told to `ask_user` instead.
* **Confirmations** (approvals inbox + panel card with the screenshot and the
  target highlighted, 5-minute TTL; reject tells the model to find another
  way): password fields, purchase / payment, delete, send (buttons by name in
  ru/en, Enter in messengers, Delete in Explorer), discard unsaved work
  ("Don't save", Alt+F4 / Ctrl+W on an unsaved title), system settings
  toggles in Settings / Control Panel / regedit / services…, and anything the
  model flags as risky.
* **Audit** — `computer.cursorStart`, one `computer.cursorStep` per step
  (typed text reduced to its length), `computer.cursorConfirm`,
  `computer.cursorConsent`, `computer.cursorFinish`.
* **Privacy** — screenshots live in memory only (last 30 of the current
  session, fetchable by the local UI), never on disk, sent only to the chosen
  backend. Each non-local backend needs a **one-time consent**
  (`NeedsConsent` → privacy note → `computer.cursorConsent`), revocable in
  Settings. The Codex CLI needs the image as a file: it is written to
  `<data>/computer/cursor-work`, deleted after the call.

### UX

* **Assistant mode** shows the cursor panel: start box (task + active
  model), consent card, then the live session — state, the model's plan
  with the current step, the last screenshot with the target highlighted,
  the step log, Pause / Resume / Stop, confirmation and question cards.
* **Settings → Cursor mode**: the source switch, ordered backends (up / down,
  availability, model override, consent), limits, "mouse movement stops",
  "minimize Orkestr".
* **Voice**: `routeUtterance` first checks `parseCursorTrigger` (ru / en,
  with or without the wake word, task after the trigger; trigger alone →
  «Что прикажете сделать, сэр?» and the next utterance is the task).
* **Jarvis phrases** («Беру управление, сэр.», «Открываю …», «Готово,
  возвращаю управление.», «Переключаюсь на …») are local strings in
  `cursor::phrases`, carried by `ComputerCursor` events and spoken by the
  voice controller. Hooks for wave 9: `CursorService::set_phrase_hook`
  (package B phrasing) and, in the UI, `useCursorStore(s => s.active)` +
  `onCursorPhrase` for the Jarvis voice orb in mode `cursor` (package A).

### Protocol

`computer.cursorStatus | cursorStart | cursorStop | cursorPause |
cursorResume | cursorAnswer | cursorConfirm | cursorConsent |
cursorSetSettings | cursorScreenshot` (`methods/cursor.rs`), event
`ComputerCursor` (ephemeral, no screenshots, no typed text). Settings in
`<data>/computer/cursor.json`.

## Verification

* Unit tests: action JSON, prompt, parser + repair, error classification,
  guardrails (purchase / delete / send / discard / system / own window /
  secrets / password / model flags), selection order, failover (sticky,
  no failover on bad output, last error), coordinate mapping on a scaled
  negative-origin monitor, downscale + PNG, phrases.
* Integration (`crates/computer/tests/cursor.rs`, mock screen + recording
  backend): end-to-end Notepad-like task, Esc / mouse / Stop aborts, step
  limit, confirmation reject → approve with approvals, refusals, questions,
  pause / resume, consent, failover event, secure desktop.
* Real desktop (`tests/cursor_desktop.rs`, `#[ignore]`, opt-in
  `ORKESTR_DESKTOP_SMOKE=1`): its own throw-away WinForms window only;
  models refuse to act (and are not even called) unless it is in front;
  clipboard checked unchanged. UIA element click → typing → close button
  verified by the form's output file; overlay visible and not captured;
  Esc / mouse aborts through the real hooks (a test-only `dwExtraInfo`
  marker makes `SendInput` count as physical); with
  `ORKESTR_SMOKE_CLAUDE=<claude.exe>` one real Claude Code run (done in 16 s).
* Desktop UI: vitest for triggers, store / consent, panel, settings reorder,
  voice routing.

## Consequences / limitations

* Windows only (`UnsupportedScreen` elsewhere).
* Elevated windows (UIPI) ignore our input; the lock screen / UAC secure
  desktop cannot be seen or driven (the session aborts).
* GDI capture: some hardware-accelerated / DRM surfaces may come out black;
  DXGI duplication can replace it behind `CursorBackend::capture`.
* Each step is a separate stateless model call with the action history in
  the prompt (no prompt caching): simple and backend-neutral, but every step
  re-sends the screenshot (≈1–2 k tokens for 1280 px).
* The CLIs start per step (~1–2 s overhead for Claude Code, more for Codex).
* Danger detection is heuristic (names, processes, model flags); the hard
  limits (own windows, secrets, abort) do not depend on the model.
