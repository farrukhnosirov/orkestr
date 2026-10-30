# ADR-0057: Fast, accurate local speech recognition and noise rejection

Status: Accepted (wave 11, v1.4.0).

## Context
Two user reports: recognition is not good enough («хочу очень быструю и
максимально качественную модель»), and the microphone very often turns
background noise / music into text that is then forwarded — the HUD showed
«смещая музыка» classified as a command and «Передал Менеджеру, сэр».

What we found:
* `whisper-cli` was spawned per utterance, re-loading the model every time.
* The shipped GPU runtime (`whisper-runtime-cuda`, the CUDA **11.8** build)
  does not contain cuBLAS; without a CUDA toolkit `ggml-cuda.dll` fails to
  load and whisper silently runs on the CPU (`no GPU found`). The «GPU»
  switch had no effect on a normal PC.
* whisper invents text for non-speech: `[музыка]`, «Продолжение следует…»,
  «Субтитры создавал DimaTorzok», «Редактор субтитров А.Синецкая…»,
  «Спасибо.», «Поехали.» — large-v3-turbo with *high* confidence and
  `no_speech_prob = 0` (its no-speech head is not usable).
* Nothing between the transcript and the AI / the Manager looked at how sure
  the recognizer was.

## Decision
**Recognizer.**
* whisper.cpp runs as a resident **`whisper-server`** on `127.0.0.1` (random
  free port), keyed by runtime + model + VAD model: the model stays loaded
  (on the GPU) between utterances. Started on demand and preloaded when the
  voice config uses whisper.cpp (`voice.getConfig` / `voice.setConfig`),
  stopped after 15 min idle, on engine switch and with the service; an
  orphan from a crash is killed by its pid file. Any server failure falls
  back to `whisper-cli` (now with `-ojf` for token probabilities).
* New package **`whisper-runtime-cuda12`** (whisper.cpp b5454 CUDA 12.4 build
  *with* cuBLAS, 685 MB, sha256-pinned). The GPU switch uses it first, then
  the old CUDA 11.8 build (kept; titled as needing the toolkit), then the CPU
  build (`-ng`).
* New package **`whisper-vad-silero`** (ggml Silero VAD v5.1.2, 0.9 MB):
  whisper.cpp drops non-speech before decoding (`--vad`).
* **Recommendation for this PC** (`VoiceEnginesResult.sttRecommendation`):
  large-v3-turbo q5_0 on an NVIDIA GPU with ≥ 3 GB VRAM (Windows x64),
  otherwise `whisper-small` (4+ cores, 8 GB RAM) or `whisper-base`.
  large-v3-turbo on the CPU is not recommended (≈20 s per command measured).
  Settings → Voice → Engines shows a card «Лучшее распознавание для этого
  ПК»: one click downloads what is missing (checksum-verified, progress) and
  switches `sttEngine` / `whisperModel` / `whisperGpu`. Defaults stay
  `whisper-base` on the CPU so nothing multi-GB is downloaded unasked.
* An **initial prompt** with the wake phrase and common command vocabulary
  (ru or en) biases whisper to «Джарвис», «открой ютуб», «громче»…; a
  transcript that reads 5+ prompt words back is a hallucination.
* Cloud engines remain opt-in with consent as before (ADR-0046).

**Noise rejection** (`VoiceEngineSettings.noiseRejection`: Off / Normal
(default) / Strict; `crates/voice/src/gate.rs`), applied by `voice.transcribe`
for the voice pipeline (an engine test shows raw text):
1. *Speech activity before any engine*: 20 ms frames, an adaptive noise floor
   (15th percentile of the clip's frame levels), speech frames ≥ 7 dB above it
   with spectral flatness < 0.42, ≥ 220 ms of them, peak ≥ −42 dBFS. Silence,
   fans/hum, white/pink/brown noise, clicks and — in the synthetic set — music
   never reach an engine (≈10 ms). Strict also needs 12 dB SNR.
2. *Decoder*: Silero VAD in whisper; segments whisper itself calls silence
   (`no_speech_prob > 0.6` and `avg_logprob < −1`) are dropped; confidence =
   exp(token-weighted mean `avg_logprob`); below exp(−1.15) ≈ 0.32 (Strict:
   exp(−0.8) ≈ 0.45) the transcript is rejected.
3. *Text*: annotations only (`[музыка]`, `(смех)`, ♪), a blacklist of whisper's
   subtitle/outro hallucinations (ru/en), runaway repetition and repeated
   sentences.
A rejection returns `TranscribeResult.rejected` (`NoSpeech` /
`Hallucination` / `LowConfidence`) with empty text; accepted text carries
`confidence` (0–100). The client treats a rejection as «не расслышал»: no
sound, no speech, a quiet HUD note for 2.5 s (in the follow-up window the
conversation just closes).

**Router gate** (`freeTextAllowed`, `voiceRoute.ts`): free text that nothing
local matched (no computer command, template, Jarvis action, near miss) only
goes to the AI conversation / planner / the Manager when the recognizer
confidence is ≥ 0.5, one- or two-word text is a question/request or ≥ 0.8,
and — in the follow-up window without a wake word — an unaddressed statement
is ≥ 0.9. Otherwise the Jarvis turn is `unheard` and nothing is sent. Known
commands, templates and Orkestr intents are not affected; typed text is never
gated.

## Measurements
Bench set (`crates/voice/examples/stt_bench.rs`): 8 Piper-synthesized
commands (4 voices), the bundled `ru_command.wav`, 4 of them mixed with
music / pink noise / TV at 8–12 dB SNR (13 speech clips); 10 noise clips
(silence, white, pink, brown, hum, clicks, 2 × music, «TV» = reversed speech
+ music, babble). i7-12700KF (other builds running), RTX 2060 SUPER 8 GB.

| Setup | Model load | Per command (mean / max) | WER (13 speech clips) | Noise clips with text |
|---|---|---|---|---|
| Before: `whisper-cli` base, CPU (default) | per call | 712 / 971 ms | 44.4 % | 10 / 10 |
| Before: «GPU» switch, CUDA 11.8 build (silently CPU), turbo | per call | ≈ 20 s | 25.1 % | 10 / 10 |
| Resident server, base, CPU, no gates | 0.5 s once | 405 / 432 ms | 52.4 % | 10 / 10 |
| Resident server, base, CPU, gated (prompt + VAD) | 0.5 s once | 454 / 496 ms | 40.3 % | 1 / 10 |
| Resident server, small, CPU, no gates | 2.1 s once | 1667 / 2066 ms | 34.1 % | 10 / 10 |
| Resident server, small, CPU, gated | 4.1 s once | 1607 / 1681 ms | 19.4 % | 1 / 10 |
| Resident server, **turbo q5_0, CUDA 12, gated** | 1.3 s once (incl. warm-up) | **188 / 202 ms** | **12.3 %** | **1 / 10** |

The remaining noise clip is the reversed-speech «TV» (speech-like by
construction); it is handled by the router gate in the follow-up window.
Turbo on the GPU without the warm-up decode: the first request took 28 s
(CUDA kernel compilation), hence the warm-up in `prewarm`. Most remaining
WER is the synthetic clips themselves («Делай громче» — Piper clips the
first consonant) and «FinSpace» vs «финспейс». Silero VAD with the default
30 ms padding clipped «Какая погода…» into «Пока, пока…»; 200 ms padding is
used. All rejected noise clips cost 0–30 ms (activity gate) except babble
(VAD + decode, ~1 s, then rejected as a repeated-sentence hallucination).

## Consequences
* The resident server keeps ~1 GB of VRAM (turbo q5_0) / ~200 MB RAM (base)
  while voice is in use; it is stopped after 15 minutes idle.
* The local server port is reachable by other local processes (loopback only,
  random port); it only transcribes audio sent to it.
* Speech quieter than the room noise floor + 7 dB is not recognized
  (Normal); Strict trades more misses for fewer false commands.
* Real TV speech is speech: outside the follow-up window a wake word is still
  required, inside it unaddressed statements need a very clear recognizer.
* Not verified live: the GPU path on other NVIDIA generations, the
  in-app download of the 685 MB CUDA package through the UI card, and real
  microphones in a noisy room.
