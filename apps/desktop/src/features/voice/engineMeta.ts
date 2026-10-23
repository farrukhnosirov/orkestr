/**
 * Speech engine metadata shared by settings, the STT session factory and
 * the speaker (ADR-0046). Mirrors `SttEngine::needs_consent` /
 * `TtsEngine::is_cloud` and the consent checks of `VoiceConfig` in the core.
 */
import type { SttEngine, TtsEngine, VoiceConfig } from "@orkestr/protocol";

export const STT_ENGINES: readonly SttEngine[] = [
  "Disabled",
  "WhisperCpp",
  "Vosk",
  "SherpaOnnx",
  "WindowsSpeech",
  "WebSpeech",
  "OpenAiWhisper",
  "Deepgram",
  "AzureSpeech",
  "GoogleCloud",
  "YandexSpeechKit",
];

export const TTS_ENGINES: readonly TtsEngine[] = [
  "System",
  "Piper",
  "WindowsOneCore",
  "Silero",
  "OpenAi",
  "AzureSpeech",
  "GoogleCloud",
  "YandexSpeechKit",
  "ElevenLabs",
];

const LOCAL_STT: readonly SttEngine[] = ["Disabled", "WhisperCpp", "Vosk", "SherpaOnnx", "WindowsSpeech"];
const CLOUD_TTS: readonly TtsEngine[] = ["OpenAi", "AzureSpeech", "GoogleCloud", "YandexSpeechKit", "ElevenLabs"];

/** Audio leaves the device (consent required). */
export function sttNeedsConsent(e: SttEngine): boolean {
  return !LOCAL_STT.includes(e);
}

/** The core runs it (`voice.transcribe`). */
export function sttRunsInCore(e: SttEngine): boolean {
  return e !== "Disabled" && e !== "WebSpeech";
}

/** Text leaves the device (consent required). */
export function ttsIsCloud(e: TtsEngine): boolean {
  return CLOUD_TTS.includes(e);
}

export function sttConsented(c: VoiceConfig, e: SttEngine): boolean {
  return !sttNeedsConsent(e) || c.sttConsent?.engine === e || (c.sttConsents ?? []).some((x) => x.engine === e);
}

export function ttsConsented(c: VoiceConfig, e: TtsEngine): boolean {
  return !ttsIsCloud(e) || (e === "OpenAi" && c.ttsConsentAt != null) || (c.ttsConsents ?? []).some((x) => x.engine === e);
}

/** `VoiceProfile.provider` ↔ engine (agent voices). */
const PROVIDERS: Record<TtsEngine, string> = {
  System: "system",
  OpenAi: "openai",
  Piper: "piper",
  Silero: "silero",
  WindowsOneCore: "windows",
  AzureSpeech: "azure",
  GoogleCloud: "google",
  YandexSpeechKit: "yandex",
  ElevenLabs: "elevenlabs",
};

export function profileProvider(e: TtsEngine): string {
  return PROVIDERS[e];
}

export function engineFromProvider(p: string): TtsEngine | null {
  return (Object.keys(PROVIDERS) as TtsEngine[]).find((e) => PROVIDERS[e] === p) ?? null;
}
