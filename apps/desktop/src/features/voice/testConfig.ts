/** VoiceConfig fields added by ADR-0046 / ADR-0054, with their defaults (test fixtures). */
import type { VoiceConfig } from "@orkestr/protocol";

export const VOICE_ENGINE_DEFAULTS: Pick<VoiceConfig, "sttFallback" | "ttsFallback" | "sttByLanguage" | "ttsByLanguage" | "sttConsents" | "ttsConsents" | "engines" | "barge" | "followUp"> = {
  sttFallback: [],
  ttsFallback: [],
  sttByLanguage: {},
  ttsByLanguage: {},
  sttConsents: [],
  ttsConsents: [],
  engines: {
    whisperModel: "whisper-base",
    whisperGpu: false,
    noiseRejection: "Normal",
    voskModel: "vosk-small-ru",
    sherpaModel: "sherpa-zipformer-small-ru-int8",
    piperVoice: "piper-ru-irina",
    windowsVoice: null,
    sileroSpeaker: "xenia",
    openaiSttModel: "gpt-4o-transcribe",
    openaiTtsModel: "gpt-4o-mini-tts",
    deepgramModel: "nova-2",
    azureRegion: "",
    azureVoice: "ru-RU-SvetlanaNeural",
    googleVoice: "",
    yandexFolderId: "",
    yandexVoice: "alena",
    elevenlabsVoice: "21m00Tcm4TlvDq8ikWAM",
    elevenlabsModel: "eleven_multilingual_v2",
  },
  barge: {
    enabled: true,
    hardPhrases: ["стоп", "хватит", "прекрати", "отмена", "перестань", "достаточно", "не надо", "stop", "enough", "cancel"],
    softPhrases: ["окей", "ок", "тихо", "тишина", "замолчи", "заткнись", "помолчи", "всё", "shut up", "quiet"],
    softCancelsActions: false,
    sensitivity: 50,
  },
  followUp: { enabled: true, windowSecs: 8, chimeOnClose: true },
};
