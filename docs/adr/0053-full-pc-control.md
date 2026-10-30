# ADR-0053: Full PC control (sound, windows, cursor, system) and a planner that knows it

Status: Accepted (wave 10, v1.3.0). Extends ADR-0035 (computer assistant).

## Context
The user wants to control every detail of the PC by voice: sound (system,
per app, microphone, devices, media), windows, cursor and mouse, keyboard,
displays, radios, files and system information. A screenshot showed two
bugs: Whisper heard «громкость 10%» as «громко с 10%», the local matcher
missed it, and the AI answered «среди моих инструментов нет управления
звуком» — the model was not told the full action vocabulary (and, behind a
CLI subscription, read its own tool list).

## Decision
* **Vocabulary** (`methods::computer`, additive): `Volume.app`, `Microphone`,
  `AudioDevice`, `MediaSeek`, `WindowResize`, `WindowOpacity`, `Desktop`,
  `MouseMove`, `MouseTo`, `MouseToElement`, `MouseHold`, `MouseDrag`,
  `MouseSpeed`, `LocateCursor`, `KeyboardLayout`, `Display`, `Radio`,
  `Query`, `Calculate`, `Alarm`, `FileCopy`, `FileSearch`,
  `EmptyRecycleBin`; `Screenshot.scope/clipboard`; new `WindowOp`s (halves,
  quarters, center, always on top, minimize / restore all) and `LevelOp`s
  (`MuteOn`, `MuteOff`, `Scale`).
* **Backend**: one extensible `ComputerBackend::pc(PcRequest) -> PcReply`
  (default `Unsupported`). Windows: Core Audio (`IAudioEndpointVolume`,
  `IAudioSessionManager2` + `ISimpleAudioVolume`, `IPolicyConfig` for the
  default device), GSMTC (now playing, seek), `SetWindowPos` /
  `WS_EX_LAYERED` / `HWND_TOPMOST`, `SPI_*MOUSESPEED`,
  `WM_INPUTLANGCHANGEREQUEST`, `SC_MONITORPOWER`, `DisplaySwitch.exe`,
  dxva2 DDC/CI brightness for external monitors, `Windows.Devices.Radios`,
  `GetSystemPowerStatus`, `SHEmptyRecycleBinW`. Geometry and level arithmetic
  live in the executor (`executor/pc_steps.rs`) so the stateful mock tests
  them. Night light, Focus / DND, resolution, scale, cursor size and power
  mode open the matching Settings page (no supported API); moving a window
  to another virtual desktop is reported as impossible (no public API).
* **Planner**: the prompt is generated from `vocab::ACTIONS` (every action,
  parameters with enum values taken from the protocol types via ts-rs,
  request → step examples). Tests fail when a protocol action is missing
  from the registry or an example does not parse. The prompt states the
  model has no tools of its own, that these actions give full PC control,
  that the text may contain recognition errors, and that an impossible
  request must say precisely which part is impossible. The Jarvis
  conversation prompt carries the generated capability summary.
* **Matcher robustness**: `text::stt_normalize` (phrase-level Whisper / Vosk
  fixes, `%` → «процентов», «на полную» → 100, «вдвое» → «в два раза»)
  is tried unless the raw text matches exactly; number slots take number
  words only; `{app}` slots reject prepositions; oblique number forms;
  spoken key letters («контрл шифт эс»). Near misses (score 0.60–0.74)
  become a local «Вы имели в виду: …?» confirmation (`didYouMean`, no
  approvals-inbox entry; the command's own risks are kept so «да» never
  skips them).
* **Safety**: `Radio`, projection modes → Confirm; `EmptyRecycleBin`,
  `KillProcess` → Dangerous (approvals inbox, audit); paste / single
  characters into a focused password field ask first; `ClickElement`
  refuses password fields; the global Stop releases a held mouse button.
* **Replies**: Jarvis-style templates per family (`assistant:say.now.*`,
  «Громкость 10 процентов, сэр.», «Окно слева, сэр.», «Курсор в центре,
  босс.»); reports are phrased from the step's JSON detail.

## Consequences
* Catalogue 1035 → 1469 commands, 6561 → 9311 phrases (4 new sets: sound,
  mouse, display, info).
* `IPolicyConfig` is undocumented (stable since Windows 7, used by the Sound
  control panel); if it ever disappears `AudioDevice` fails with a clear
  error and the rest keeps working.
* Wi-Fi / Bluetooth switching may be refused by Windows privacy settings
  (reported as Forbidden with the Settings path).
