# ADR-0052: Orb = Jarvis reactor (one circle)

Status: Accepted (wave 9, v1.2.0). Supersedes the sphere part of ADR-0045
(the Agent Hub, the orb window and its channel stay as decided there).

## Context
On the wake word Orkestr showed a plain dark box («Слушаю…») next to a
separate floating sphere. The user asked for a futuristic Jarvis-style circle
that reacts to the voice, with a good-looking window for the recognized text.
Two circles (the sphere and a voice indicator) would compete for attention.

## Decision
* **One circle.** The floating orb *is* the Jarvis reactor
  (`features/voice/jarvis/`: `JarvisReactor` canvas renderer, `JarvisVoice`
  card with a glass transcript / reply panel, `model.ts` state → visual
  parameters, `signal.ts` mic / TTS level broadcast). `OrbSphere` is removed.
  The in-app voice HUD uses the same components.
* **States**: rest (idle / working / approval / error / offline / stopped),
  wake, listening (reacts to the mic level), thinking, speaking (reacts to
  the TTS output level through `tts/outputTap.ts`), done, error.
* **Modes** on the ring: `command`, `chat` (ADR-0049 conversation mode) and
  `cursor` (ADR-0050, while the AI controls the cursor). The voice store's
  `assistantMode` is kept in step by `features/jarvis/modeSync.ts` and is sent
  to the orb window in the channel state.
* **Size**: `OrbPrefs.voiceSize` 120–180 px (default 150); the orb window grows
  for the voice card (`orb_set_layout`) and shrinks back afterwards.
* Theme tokens and Performance mode are respected (the renderer drops to a
  static frame in reduced motion / performance mode).

## Consequences
* Screenshots: `docs/screenshots/jarvis-voice/`, script
  `scripts/smoke/jarvis-voice-screenshots.mjs` (isolated instance, mock voice).
* Cursor phrases are spoken through the voice controller (`setCursorSpeaker`),
  so the circle shows them as replies.
