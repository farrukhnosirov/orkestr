# ADR-0054: Voice barge-in and continuous conversation

Status: Accepted (wave 10, v1.3.0).

## Context
The user wants to interrupt the assistant at any moment by voice («окей,
прекрати, стоп, хватит, заткнись, замолчи…») and to talk to it like a person:
after a reply, just keep talking without the wake word.

## Decision
**Barge-in: a stop-phrase spotter in the core, fed by the one microphone.**
* While the assistant speaks, works (`thinking`: a streamed AI reply, an AI
  call, a multi-step computer command) or controls the cursor, the webview's
  single `MicCapture` (echo cancellation + noise suppression on) records 16 kHz
  PCM and sends ~100 ms chunks to the local core (`voice.bargeStart` /
  `voice.bargeFeed` / `voice.bargeStop`, local-only methods). No second
  microphone is opened; audio never leaves the machine and is dropped after
  each chunk.
* `crates/voice/src/barge.rs`: a **Vosk grammar recognizer** on
  `vosk-small-ru` restricted to the configured phrases plus `[unk]`
  (`orkestr-platform::vosk::VoskStream`, ~3 ms per 100 ms chunk). A grammar
  forces any sound onto its phrases («Оркестр…» → «окей»), so a hit also needs:
  an energy gate (speech within the last 700 ms, sensitivity-dependent
  threshold), a **pause after the phrase** (200 ms at sensitivity 50; +120 ms
  for soft phrases, which are short common words), a short utterance (≤ 1.6 s
  voiced, ≤ 5 words, ≤ 2 words besides the phrase), a minimum confidence on
  final results, and **self-echo rejection** (a phrase contained in the
  sentence being spoken is never a hit; the client re-checks). A
  full-vocabulary second decoder was tried as a verifier and rejected: ~20×
  the CPU (up to 110 ms per 100 ms chunk). Without the Vosk model the session
  falls back to short voiced windows transcribed by an installed offline engine
  (whisper.cpp / sherpa-onnx); with neither, barge-in is unavailable (wake
  word, push-to-talk, Esc and the circle still interrupt).
* Measured (`crates/voice/tests/barge.rs`, Piper-synthesized «Стоп.»,
  100 ms chunks): detection **255 ms after the word ends** (≈250 ms of audio +
  3 ms decode); the first session loads the model in ~0.5–1.4 s.
* **Hard** phrases (стоп, хватит, прекрати, отмена, перестань, достаточно,
  не надо, stop, enough, cancel…) silence speech and cancel the streamed reply
  (`jarvisSession.cancel`), the running computer command (`computer.stop`) and
  the pending confirmation; the late result of that turn is dropped (turn
  counter). **Soft** phrases (окей, ок, тихо, тишина, замолчи, заткнись,
  помолчи, всё…) only silence speech unless *soft phrases cancel actions* is
  set. Any stop phrase aborts cursor control. Nothing is said; the circle plays
  a short contraction ("interrupted") and listens for the next command for 5 s,
  with 0.8 s of pre-roll so «стоп, открой ютуб» works (the leading stop phrase
  is stripped from the transcript).
* The wake word and push-to-talk while the assistant works interrupt it (hard)
  and start a new command; Esc or a click on the circle while it speaks
  silences speech.

**Continuous conversation.** After a voice-initiated reply has been spoken
(or shown, with spoken replies off) the controller re-opens listening without
the wake word after 300 ms for the follow-up window (default 8 s, 3–30 s). VAD
start detection ends the countdown and the capture runs until the user stops.
The next utterance goes through the same session (conversation memory
continues), then the window re-opens. Silence closes quietly (optional soft
chime, no spoken goodbye); «спасибо, всё», «пока», «на этом всё», «отбой»,
«that's all», a stop phrase alone or the assistant's own echo close at once.
Typed commands never open the microphone. The circle (orb + HUD) shows a thin
countdown ring and «Слушаю — просто продолжайте».

**Settings** (`VoiceConfig.barge`, `VoiceConfig.followUp`; Settings → Voice →
Conversation): barge-in on/off (default on), editable hard / soft phrase lists
(ru/en defaults), what «окей» does, sensitivity; follow-up on/off (default on),
window length, chime.

## Consequences
* While the assistant speaks or works the microphone is open with the purpose
  "Stop phrases" (the indicator shows it); with barge-in off it is not.
* English phrases are matched in transcripts (follow-up / fallback windows)
  but not by the Russian grammar model (not in its vocabulary).
* A grammar spotter can still false-trigger on a short word followed by a
  pause; soft phrases only silence speech by default, which keeps such errors
  cheap. Sensitivity trades accidental stops for quieter commands.
