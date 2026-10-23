import type { SttEngine } from "@orkestr/protocol";
import { describe, expect, it } from "vitest";

import { engineNeedsPcm, SttError, startStt } from "./engines";

describe("engineNeedsPcm", () => {
  it("opens the microphone for every core engine", () => {
    const core: SttEngine[] = ["OpenAiWhisper", "WhisperCpp", "Vosk", "SherpaOnnx", "WindowsSpeech", "Deepgram", "AzureSpeech", "GoogleCloud", "YandexSpeechKit"];
    for (const e of core) expect(engineNeedsPcm(e)).toBe(true);
  });
  it("does not for WebSpeech / Disabled", () => {
    expect(engineNeedsPcm("WebSpeech")).toBe(false);
    expect(engineNeedsPcm("Disabled")).toBe(false);
  });
  it("Disabled fails with a dedicated code", () => {
    try {
      startStt({ engine: "Disabled", lang: "ru", mic: null, autoEnd: false });
      expect.unreachable();
    } catch (e) {
      expect(e).toBeInstanceOf(SttError);
      expect((e as SttError).code).toBe("disabled");
    }
  });
});
