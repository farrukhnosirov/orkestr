# ADR-0017: Local wake-word detection, with push-to-talk as the default

- **Status:** Accepted (model and runtime choice is conditional on SP-15)
- **Date:** 2026-10-02
- **Phase:** 3 (Phase 1 defines `voice.*` stubs and the `Microphone` trait, RR-C1)
- **Spec:** §31 (Battery Saver), §33 (voice first-class; wake word configurable, local where possible, no continuous mic stream to the cloud without explicit consent), §46 (Microphone, GlobalHotkeys, Power), §56 (privacy), §62.8, §63G
- **Review:** RR-C1
- **Spike:** SP-15 · **Risk:** R-08

## Context

§63G: "Jarvis, what's happening with Schoolify?" must work hands-free. §33 forbids streaming the microphone to the cloud continuously without explicit consent. §62.8 names wake-word battery and CPU cost as a risk. Wake words must be configurable.

## Decision drivers

1. Privacy: audio never leaves the device before the wake word fires, and only with consent afterwards.
2. CPU ≤ 2% of one core on average while listening; nothing at all while disabled or on battery saver.
3. Permissive license that allows bundling (RR-G18).
4. Runs in Rust inside the core (so it also works headless), with no Python runtime.
5. Custom wake words.

## Considered options

| Option | Privacy | Cost | License / notes | Verdict |
|---|---|---|---|---|
| **openWakeWord-style ONNX models (melspectrogram → embedding → per-word classifier) on `ort` (ONNX Runtime) or `tract`, gated by VAD** | Local | Low (VAD gate) | Apache-2.0 code; pre-trained models include "hey jarvis". Check each model's license (SP-15) | **Chosen** |
| Picovoice Porcupine | Local | Very low | Proprietary, needs an access key per user and device limits | Rejected as default; possible optional adapter |
| Vosk keyword spotting (Kaldi) | Local | Medium | Apache-2.0; large models | Fallback candidate |
| Whisper (whisper.cpp) running continuously | Local | High | MIT | Rejected for wake (CPU); **used for STT after wake** |
| Cloud streaming STT always on | Cloud | — | — | Forbidden without consent (§33) |

## Decision

- **Default interaction is push-to-talk**, through a global hotkey (`PlatformAdapter::hotkeys`, default `Ctrl+Alt+Space`) and the HUD mic button. The wake word is **off by default** and is enabled in onboarding step 9 or in Settings.
- **Pipeline (`orkestr-core::voice`; platform capture in `orkestr-platform::microphone`):**
  ```mermaid
  flowchart LR
    Mic[cpal capture 16 kHz mono] --> RB[Ring buffer 2 s]
    RB --> VAD{"Silero VAD / WebRTC VAD<br/>speech?"}
    VAD -- no --> Sleep[discard frames]
    VAD -- yes --> WW["Wake-word classifier ONNX<br/>80 ms frames"]
    WW -- score above threshold for N frames --> Cap["Command capture until VAD silence 800 ms, max 15 s"]
    Cap --> STT{STT provider}
    STT -- local default --> Whisper["whisper.cpp small/base via whisper-rs"]
    STT -- cloud only with consent --> Cloud[Provider STT]
    Whisper --> Intent["Intent: text → Manager / direct command"]
    Intent --> RPC["RPC methods, e.g. emergency.stopProject"]
  ```
- **Traits:**
  ```rust
  pub trait Microphone: Send + Sync {            // orkestr-platform
      fn devices(&self) -> Result<Vec<AudioDevice>, PlatformError>;
      fn open(&self, device: Option<&str>, cfg: CaptureConfig) -> Result<Box<dyn AudioStream>, PlatformError>; // 16 kHz mono i16
  }
  pub trait WakeWordDetector: Send {             // orkestr-core::voice
      fn keywords(&self) -> &[Keyword];
      fn process(&mut self, frame: &[i16]) -> Option<WakeHit>;   // WakeHit { keyword, score }
  }
  pub trait SpeechToText: Send + Sync { async fn transcribe(&self, pcm: &[i16], lang: Option<&str>) -> Result<Transcript, VoiceError>; }
  pub trait TextToSpeech: Send + Sync { async fn speak(&self, text: &str, voice: &VoiceProfile) -> Result<AudioClip, VoiceError>; }
  ```
- **Custom wake words:** pick from the bundled models ("hey jarvis", "orkestr" if trained). Custom words are trained offline with the openWakeWord synthetic-data pipeline (documented in `docs/voice/`). That training is out of app scope in Phase 3, and users import the `.onnx` file. Each keyword has its own sensitivity setting.
- **Power policy:** `PlatformAdapter::power().on_battery()` and Battery Saver mode pause wake-word listening (push-to-talk still works). On Windows, a `SHQueryUserNotificationState` presentation or fullscreen state also pauses it.
- **Privacy UX:** a persistent mic indicator in the HUD and title bar while listening. A `VoiceCommand` event is logged (transcript only, never audio). Audio buffers are dropped right after STT. Cloud STT needs a per-provider consent record (`consents` table) and is labelled.
- **Intent routing:** direct commands (stop, show, open) go through a small local grammar with fuzzy project and agent name matching. Everything else is sent to the Manager as text (`manager.send`). Destructive intents ("stop all") need a spoken or clicked confirmation, except emergency stop, which runs immediately (§52).

## Consequences

- **Positive:** private by design; low idle cost; works in headless `orkestrd` on a machine with a mic; voice uses the same RPC as every other client.
- **Negative:** ONNX Runtime adds about 10–20 MB of native libraries per platform. `tract` (pure Rust) is preferred if SP-15 shows it runs the models within budget.
- **Negative:** false accepts and rejects need tuning. SP-15 targets false accepts ≤ 1 per 10 h of background audio and false rejects ≤ 10% at 1 m.

## Implementation notes (Phase 0 foundation, 2026-10-02)

Not implemented yet. `orkestr-platform` exposes a `Microphone` trait (`list_devices()`) whose only implementation returns `PlatformError::NotSupported` on every OS until Phase 3. SP-15 (desk study) has not run.
