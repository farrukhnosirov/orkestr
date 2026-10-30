# ADR-0056: Wake listener reliability, several wake phrases, bounded follow-up

Status: Accepted (wave 11, v1.4.0). Refines ADR-0017 (wake word) and ADR-0054
(continuous conversation).

## Context
User reports after v1.3.0:
1. «Sometimes "Джарвис" does nothing; maybe something crashes in the
   background; I have to restart Orkestr.»
2. «I want several wake words.»
3. «I asked not to repeat the wake word for ~5–6 s, not forever. Listening to
   me all the time after the first wake word is uncomfortable.»

Causes found in the code:
* The wake listener had no way back from a dead stream. A removed or
  re-enumerated device, sleep/resume or a suspended `AudioContext` left the
  worklet without input: no segments, no error, status still «listening».
  A failed start stayed in `error` until settings changed.
* The pipeline phase could stay in `listening` / `transcribing` forever (an
  STT session whose auto-end never fired, a `finish()` that never settled,
  a `getUserMedia` that never answered). Wake segments are ignored in those
  phases and `startListening` refuses while a session exists, so the wake word
  was dead until a restart.
* Follow-up: every reply re-armed the window, including replies to noise. Any
  energy in the window (`onSpeech`) cancelled the window limit; the recognizer
  then turned the noise into text (Whisper hallucinations such as «Продолжение
  следует…»), the assistant answered it — and re-armed again. Chains never
  ended.

## Decision
**Watchdog (`wake/health.ts`, pure; `installVoiceWatchdog` in the controller).**
* Heartbeat: every worklet message. Status `listening` with no audio for 6 s
  → restart (close the microphone, open a fresh stream). `MicCapture.onDead`
  (track `ended`, `AudioContext` closed / not resumable) restarts at once.
* `error` (mic failed to open) → retry with backoff 1 s, 3 s, 10 s, 30 s, 60 s.
* A start stuck in `starting`/`restarting` for 18 s, or a wanted listener that
  is `off`, → restart. A clock gap ≥ 45 s between checks (sleep) → restart.
* `devicechange` (debounced 1.5 s) restarts a listener on the default device or
  one in `error`; `visibilitychange` to visible runs a check at once.
* Restarts never cut a command capture, enrollment or the Settings meter (the
  next check retries). Each restart is logged (`[voice] wake listener restart
  (reason)`) and counted (`__orkestrDebug.voice.state().wakeRestarts`).
* Phase watchdog: a phase older than its limit (listening 30 s, transcribing
  75 s, confirm 120 s, error 60 s, result 180 s, thinking 300 s) is reset to
  idle (`cancelVoice`), dropping the late result of that turn.
* Timeouts on the chain: opening the microphone 10 s (a late stream is
  closed), `SttSession.finish()` 70 s, the agent list for the parser 3 s.
* Settings show the state quietly: «Слушаю «Джарвис», «Jarvis»» /
  «Перезапуск слушателя…» / «Ошибка».

**Several wake phrases.**
* `WakeWordConfig.phrases: string[]` (1..=8, unique ignoring case, primary
  first); `phrase` stays and always equals `phrases[0]` for older clients.
  `VoiceConfig::migrate()` (core: on load and on `voice.setConfig`) fills the
  list from `phrase`, drops blanks/duplicates and clamps the follow-up bounds.
  Default: «Джарвис»; Settings offer «Эй, Джарвис», «Окей, Джарвис», «Jarvis».
* Audio: voice samples are stored **per phrase** (`orkestr.voice.wakeTemplates.v2`
  = `{ sets: [{ phrase, samples }] }`; the v1 single set is read and migrated on
  the next save). `WakeDetector` runs one DTW spotter per recorded phrase with a
  shared cooldown; the best match relative to its own threshold wins. Phrases
  that sound alike (`Jarvis`/`Джарвис`) share one recording.
* Text: `wake/phrases.ts` expands phrases into recognizer spellings
  («джервис», «жарвис», «дарвис», «jarvis»…, lead-ins эй/hey, окей/ok) plus a
  one-edit fuzzy match; these are stripped from commands, and the wake phrase
  alone said inside a follow-up window starts a fresh wake-word capture.

**Bounded follow-up.**
* `FollowUpConfig.windowSecs` 3..=10, default 6 (was 3..=30, default 8); new
  `maxTurns` 1..=10, default 3.
* One window after the assistant finishes speaking. The window is a hard
  controller-side limit (window + 400 ms) independent of the engine; speech
  inside it extends the capture only until the utterance ends.
* Noise, fillers («э», «угу») and recognizer hallucinations close the window
  with the soft chime: no reply, no new window.
* The window re-opens only after an answer with outcome ok / sent to Manager /
  needs confirmation, and only `maxTurns` follow-up turns in a row; then the
  wake word is needed. A wake-word or push-to-talk turn starts a new count;
  typed commands never open it.

## Consequences
* A dead microphone recovers in ≤ 6–8 s without restarting Orkestr; a
  hung recognition frees the pipeline in ≤ 75 s.
* Each extra phrase that sounds different needs its own samples (3–5);
  unrecorded phrases still count in transcripts.
* Not covered: detection quality of the DTW spotter itself (false rejects in
  noise); an STT-verified second stage could follow.
