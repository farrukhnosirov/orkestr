/** Settings → Voice → Speech engines + Voice models (ADR-0046). */
import type { VoiceConfig, VoiceEngineInfo, VoiceModelsResult, VoicePackageInfo } from "@orkestr/protocol";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { applyLanguagePreference } from "../../i18n";
import { engineFromProvider, sttConsented, sttNeedsConsent, ttsConsented, ttsIsCloud } from "./engineMeta";
import { EnginesPanel } from "./settings/EnginesPanel";
import { ModelsPanel } from "./settings/ModelsPanel";
import { useVoiceStore } from "./store";
import { VOICE_ENGINE_DEFAULTS } from "./testConfig";
import { optionsForProfile, type SpeakOptions } from "./tts/speaker";

const calls: Array<{ method: string; params: unknown }> = [];
let handler: (method: string, params: unknown) => unknown = () => ({});
const fake = {
  call: vi.fn(async (method: string, params: unknown) => {
    calls.push({ method, params });
    return handler(method, params);
  }),
};
vi.mock("../../core/client", () => ({ getClient: () => fake, maybeClient: () => fake }));

const CONFIG = {
  enabled: true,
  sttEngine: "WhisperCpp",
  sttConsent: null,
  ttsEngine: "Piper",
  ttsConsentAt: null,
  cloudProviderId: null,
  language: "ru-RU",
  inputDeviceId: null,
  pushToTalkHotkey: "Ctrl+Space",
  spokenReplies: true,
  naturalReplies: false,
  systemVoice: null,
  cloudVoice: "alloy",
  ratePct: 100,
  pitchPct: 100,
  wake: { enabled: false, phrase: "Jarvis", phrases: ["Jarvis"], sensitivity: 50, cpuBudgetPct: 2, pauseOnBatterySaver: true },
  notifications: {
    enabled: true,
    quietHours: { enabled: false, start: "22:00", end: "08:00" },
    taskCompleted: true,
    taskFailed: true,
    approvalNeeded: true,
    buildFailed: true,
    budgetThreshold: true,
    deployFinished: true,
  },
  sound: { enabled: true, volumePct: 55, preset: "Auto", muteInDnd: true },
  agentVoices: {},
  ...VOICE_ENGINE_DEFAULTS,
} satisfies VoiceConfig;

function engine(p: Partial<VoiceEngineInfo>): VoiceEngineInfo {
  return {
    stt: null,
    tts: null,
    name: "x",
    offline: true,
    cloud: false,
    streaming: false,
    languages: ["*"],
    russian: true,
    quality: "High",
    speed: "High",
    status: "Ready",
    detail: null,
    packages: [],
    key: null,
    keyConfigured: false,
    needsConsent: false,
    consented: true,
    verification: "Tested",
    options: [],
    ...p,
  };
}

const ENGINES = {
  stt: [
    engine({ stt: "WhisperCpp", options: [{ id: "whisper-base", label: "Whisper base", language: "*", available: true }] }),
    engine({ stt: "Deepgram", offline: false, cloud: true, status: "NeedsKey", key: "Deepgram", needsConsent: true, consented: false, verification: "RequestShapeVerified" }),
  ],
  tts: [engine({ tts: "Piper", options: [{ id: "piper-ru-irina", label: "Irina", language: "ru", available: true }] })],
  configuredKeys: [],
};

function pkg(p: Partial<VoicePackageInfo>): VoicePackageInfo {
  return {
    id: "whisper-base",
    title: "Whisper base",
    kind: "SttModel",
    stt: "WhisperCpp",
    tts: null,
    languages: ["*"],
    downloadBytes: 1000,
    installedBytes: 0,
    state: "NotInstalled",
    progressBytes: 0,
    error: null,
    recommended: true,
    dependsOn: [],
    sha256: ["a".repeat(64)],
    supported: true,
    license: "MIT",
    source: "huggingface.co",
    ...p,
  };
}

beforeEach(async () => {
  calls.length = 0;
  useVoiceStore.setState({ config: CONFIG });
  await act(async () => {
    await applyLanguagePreference("ru");
  });
});
afterEach(async () => {
  cleanup();
  await act(async () => {
    await applyLanguagePreference("en");
  });
});

describe("engine metadata", () => {
  it("knows which engines send data off the device", () => {
    expect(sttNeedsConsent("WhisperCpp")).toBe(false);
    expect(sttNeedsConsent("Vosk")).toBe(false);
    expect(sttNeedsConsent("Deepgram")).toBe(true);
    expect(ttsIsCloud("Piper")).toBe(false);
    expect(ttsIsCloud("ElevenLabs")).toBe(true);
    expect(sttConsented(CONFIG, "Deepgram")).toBe(false);
    expect(sttConsented({ ...CONFIG, sttConsents: [{ engine: "Deepgram", grantedAt: "2026-10-23T00:00:00Z" }] }, "Deepgram")).toBe(true);
    expect(ttsConsented({ ...CONFIG, ttsConsentAt: "2026-10-23T00:00:00Z" }, "OpenAi")).toBe(true);
    expect(engineFromProvider("piper")).toBe("Piper");
    expect(engineFromProvider("nope")).toBeNull();
  });

  it("agent voices can use local core engines", () => {
    const base: SpeakOptions = { engine: "Piper", lang: "ru-RU", systemVoice: null, cloudVoice: "alloy", style: null, ratePct: 100, pitchPct: 100 };
    const o = optionsForProfile({ provider: "piper", voice: "piper-ru-denis", language: "auto", style: null }, base);
    expect(o).toMatchObject({ engine: "Piper", coreVoice: "piper-ru-denis", explicitEngine: false });
    const w = optionsForProfile({ provider: "windows", voice: "Microsoft Pavel", language: "auto", style: null }, base);
    expect(w).toMatchObject({ engine: "WindowsOneCore", explicitEngine: true });
    // A cloud voice that is not the selected engine falls back to the system voice.
    expect(optionsForProfile({ provider: "elevenlabs", voice: "abc", language: "auto", style: null }, base).engine).toBe("System");
  });
});

describe("EnginesPanel", () => {
  it("renders engine cards in Russian and stores API keys through the vault RPC", async () => {
    handler = (m) => (m === "voice.listEngines" || m === "voice.setEngineKey" ? ENGINES : {});
    render(<EnginesPanel config={CONFIG} patch={() => undefined} />);
    expect(await screen.findByTestId("engine-stt-WhisperCpp")).toBeTruthy();
    expect(screen.getByText("Нужен API-ключ")).toBeTruthy();
    expect(screen.getByText("Формат запроса проверен")).toBeTruthy();
    const input = screen.getByLabelText("API-ключ");
    fireEvent.change(input, { target: { value: "dg-123" } });
    fireEvent.click(screen.getByText("Сохранить ключ"));
    await waitFor(() => expect(calls.some((c) => c.method === "voice.setEngineKey")).toBe(true));
    expect(calls.find((c) => c.method === "voice.setEngineKey")?.params).toEqual({ key: "Deepgram", apiKey: "dg-123" });
  });

  it("speaks a sample with exactly the tested engine", async () => {
    handler = (m) => (m === "voice.listEngines" ? ENGINES : m === "voice.synthesize" ? { audioBase64: "UklGRg==", mimeType: "audio/wav", engine: "Piper" } : {});
    const play = vi.fn(async () => undefined);
    vi.stubGlobal(
      "Audio",
      class {
        onended: (() => void) | null = null;
        play = play;
      },
    );
    URL.createObjectURL = vi.fn(() => "blob:x");
    URL.revokeObjectURL = vi.fn();
    render(<EnginesPanel config={CONFIG} patch={() => undefined} />);
    const card = await screen.findByTestId("engine-tts-Piper");
    fireEvent.click(card.querySelector("button") as HTMLButtonElement);
    await waitFor(() => expect(play).toHaveBeenCalled());
    const synth = calls.find((c) => c.method === "voice.synthesize")?.params as { engine: string; language: string };
    expect(synth).toMatchObject({ engine: "Piper", language: "ru-RU" });
    vi.unstubAllGlobals();
  });

  it("asks for consent before adding a cloud engine to the fallback chain", async () => {
    handler = (m) => (m === "voice.listEngines" ? ENGINES : m === "voice.setConsent" ? { config: CONFIG, cloudProviders: [] } : {});
    const patch = vi.fn();
    render(<EnginesPanel config={CONFIG} patch={patch} />);
    await screen.findByTestId("engine-stt-WhisperCpp");
    const selects = screen.getAllByRole("combobox");
    const add = selects.find((s) => (s as HTMLSelectElement).getAttribute("aria-label") === "Добавить движок…") as HTMLSelectElement;
    fireEvent.change(add, { target: { value: "Deepgram" } });
    await waitFor(() => expect(patch).toHaveBeenCalled());
    expect(calls.find((c) => c.method === "voice.setConsent")?.params).toEqual({ scope: "SpeechRecognition", granted: true, engine: "Deepgram", keepSelection: true });
  });
});

describe("best recognition for this PC (ADR-0057)", () => {
  it("downloads what is missing, then switches to whisper large-v3-turbo on the GPU", async () => {
    const rec = { engine: "WhisperCpp" as const, model: "whisper-large-v3-turbo-q5", gpu: true, packages: ["whisper-runtime-cuda12", "whisper-large-v3-turbo-q5"], downloadBytes: 1_259_000_000, gpuName: "NVIDIA GeForce RTX 2060 SUPER, 8 GB", active: false };
    let polls = 0;
    handler = (m) => {
      if (m === "voice.listEngines") return { ...ENGINES, sttRecommendation: rec };
      if (m === "voice.listModels") {
        polls += 1;
        const state = polls > 1 ? "Installed" : "Downloading";
        return { root: "x", diskUsageBytes: 0, packages: rec.packages.map((id) => pkg({ id, state, downloadBytes: 100, progressBytes: polls > 1 ? 100 : 40 })) };
      }
      return {};
    };
    const patch = vi.fn();
    render(<EnginesPanel config={CONFIG} patch={patch} />);
    const card = await screen.findByTestId("stt-recommendation");
    expect(card.textContent).toContain("RTX 2060 SUPER");
    fireEvent.click(card.querySelector("button") as HTMLButtonElement);
    await waitFor(() => expect(patch).toHaveBeenCalled(), { timeout: 4000 });
    expect(calls.filter((c) => c.method === "voice.downloadModel").map((c) => c.params)).toEqual([{ id: "whisper-runtime-cuda12" }, { id: "whisper-large-v3-turbo-q5" }]);
    const next = (patch.mock.calls[0]?.[0] as (c: VoiceConfig) => VoiceConfig)(CONFIG);
    expect(next.sttEngine).toBe("WhisperCpp");
    expect(next.engines).toMatchObject({ whisperModel: "whisper-large-v3-turbo-q5", whisperGpu: true });
  });

  it("is hidden when the recommended setup is already in use", async () => {
    handler = (m) => (m === "voice.listEngines" ? { ...ENGINES, sttRecommendation: { engine: "WhisperCpp", model: "whisper-base", gpu: false, packages: [], downloadBytes: 0, active: true } } : {});
    render(<EnginesPanel config={CONFIG} patch={() => undefined} />);
    await screen.findByTestId("engine-stt-WhisperCpp");
    expect(screen.queryByTestId("stt-recommendation")).toBeNull();
  });
});

describe("ModelsPanel", () => {
  it("lists packages, starts downloads and shows progress", async () => {
    let state: VoiceModelsResult = { packages: [pkg({}), pkg({ id: "piper-ru-irina", title: "Piper Irina", kind: "TtsVoice", stt: null, tts: "Piper", state: "Installed", installedBytes: 64_000_000 })], diskUsageBytes: 64_000_000, root: "C:/x/voice/models" };
    handler = (m) => {
      if (m === "voice.downloadModel") state = { ...state, packages: [pkg({ state: "Downloading", progressBytes: 500 }), state.packages[1] as VoicePackageInfo] };
      if (m === "voice.verifyModel") return { ok: true, files: 2, mismatches: [] };
      return state;
    };
    render(<ModelsPanel pollMs={20} />);
    expect(await screen.findByTestId("model-whisper-base")).toBeTruthy();
    expect(screen.getByText("Установлено")).toBeTruthy();
    fireEvent.click(screen.getByText("Скачать"));
    const bar = await screen.findByRole("progressbar");
    expect(bar.getAttribute("aria-valuenow")).toBe("50");
    fireEvent.click(screen.getByText("Проверить"));
    expect(await screen.findByText(/Контрольные суммы совпадают/)).toBeTruthy();
  });
});
