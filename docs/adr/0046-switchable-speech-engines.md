# ADR-0046: Switchable speech engines and the voice model manager

- **Status:** Accepted
- **Date:** 2026-10-23
- **Phase:** Wave 8 (user request 5)
- **Spec:** §33 (voice first-class, local where possible, consent for cloud), §56 (privacy), §46 (platform layer)
- **Builds on:** ADR-0017 (wake word, push-to-talk, consent model)

## Context

Phase 3 shipped three STT options (off, Web Speech, OpenAI Whisper) and two
TTS options (system voices, OpenAI). Both cloud-only recognizers send audio
off the device, so Russian voice control needed a consent before it worked
at all. The user asked for switchable engines, every free engine that speaks
Russian installed and working, and API-key settings for the paid ones.

## Decision

1. **Engines run in the core** (new crate `orkestr-voice`), behind one entry
   point per direction (`VoiceService::transcribe` / `synthesize`) that
   dispatches on the protocol enums `SttEngine` / `TtsEngine` (extended
   additively). The webview keeps capture (16 kHz WAV from the existing
   worklet) and playback; `WebSpeech` and the webview `System` voices stay
   client-side.

   | Engine | Kind | How it runs | Russian | Status on the reference machine |
   |---|---|---|---|---|
   | whisper.cpp | STT, offline | `whisper-cli.exe` (release b5454, CPU; optional CUDA 11.8 build) | yes (multilingual) | installed + tested |
   | Vosk | STT, offline | `libvosk.dll` 0.3.45 loaded at run time (FFI in `orkestr-platform::vosk`) | `vosk-model-small-ru-0.22` | installed + tested |
   | sherpa-onnx | STT, offline | `sherpa-onnx.exe` 1.13.8, streaming zipformer (Vosk ru export) | yes | on demand (manually verified) |
   | Windows Speech | STT, offline | `System.Speech` dictation via PowerShell | needs a ru-RU recognizer | en-US only here → explicit error for ru |
   | Piper | TTS, offline | `piper.exe` 2023.11.14-2 + ru_RU irina/denis/dmitri/ruslan | yes | installed + tested |
   | Windows OneCore | TTS, offline | WinRT `SpeechSynthesizer` via PowerShell → WAV | Irina, Pavel | detected + tested |
   | Silero | TTS, offline | Python + PyTorch (`ORKESTR_SILERO_PYTHON`) | yes | not installed (see below) |
   | OpenAI, Deepgram, Azure, Google, Yandex SpeechKit, ElevenLabs | cloud | REST via `reqwest` | yes | request shapes verified against a mock |

2. **Routing:** `VoiceConfig` gains `sttByLanguage` / `ttsByLanguage`
   (primary language subtag → engine), `sttFallback` / `ttsFallback`
   (ordered, ≤ 8) and `engines` (per-engine options). The core tries the
   language default (or the selected engine), then the fallback chain; the
   client falls back to the system voice last. Results report the engine
   that answered.
3. **Consent:** local engines need none. Every cloud engine needs its own
   consent (`sttConsents` / `ttsConsents`, server-owned; the legacy single
   slots still count). `voice.setConfig` rejects a configuration that routes
   to any unconsented engine; revoking prunes the engine from the selection
   and chains. `keepSelection` grants consent without switching the primary.
4. **Keys:** paid-engine API keys are global secrets in the secret vault
   (`voice.<vendor>.apiKey`), written by the local-only
   `voice.setEngineKey`, never returned over RPC and never stored in the
   config document. Keys travel in headers only. Non-secret settings
   (Azure region, Yandex folder id, voices, models) live in `engines`.
   OpenAI without a voice key keeps using the user's OpenAI provider with
   the provider-bound consent of ADR-0017.
5. **Model manager:** a pinned catalog (URL, size, SHA-256 from GitHub
   release digests / Hugging Face LFS ids, or computed on 2026-10-23) under
   `<ORKESTR_HOME>/voice/models/<id>`. Downloads run one at a time with
   progress, are hashed while streaming and rejected on any size/hash
   mismatch before unpacking (zip via the `zip` crate with zip-slip checks,
   `.tar.bz2` via the system `tar`), archives are deleted after unpacking,
   and an install manifest of per-file SHA-256 backs `voice.verifyModel`.
   Dependencies (runtimes) install first. Methods: `voice.listModels`,
   `downloadModel`, `cancelDownload`, `removeModel` (local-only, audited),
   `verifyModel`.
6. **Silero** needs PyTorch (≈ 200 MB download, ≈ 1 GB installed for the CPU
   wheel) plus a Python runtime the app does not ship. It is under the 2 GB
   limit but a Python dependency in a desktop app is fragile, so it is
   optional: the engine runs when `ORKESTR_SILERO_PYTHON` points to a Python
   with `torch`, and is reported as Unavailable otherwise.

## Consequences

- Russian voice control works fully offline out of the box once the
  recommended packages are downloaded (~620 MB: whisper base, Vosk small ru,
  Piper runtime + 4 ru voices).
- The native runtimes are Windows x64 builds; on other OSes those engines
  report Unavailable (models are cross-platform, runtimes are not yet).
- Engine e2e tests need installed packages (`ORKESTR_VOICE_TEST_DIR`) and
  are skipped otherwise; cloud engines are covered by request-shape tests.
- Every helper process gets argument vectors (no shell), no console window,
  a timeout, and temp WAV files that are deleted right after the call.
