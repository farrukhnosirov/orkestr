/** Voice UI renders in Russian and English with the core-owned config (no mic needed). */
import type { VoiceConfig } from "@orkestr/protocol";
import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { applyLanguagePreference, i18n } from "../../i18n";
import { SoundSection } from "./settings/SoundSection";
import { useVoiceStore } from "./store";
import { MicIndicator } from "./ui/MicIndicator";
import { VoiceHud } from "./ui/VoiceHud";
import { executeIntent } from "./pipeline/executor";

const CONFIG: VoiceConfig = {
  enabled: true,
  sttEngine: "Disabled",
  sttConsent: null,
  ttsEngine: "System",
  ttsConsentAt: null,
  cloudProviderId: null,
  language: "auto",
  inputDeviceId: null,
  pushToTalkHotkey: "Ctrl+Space",
  spokenReplies: false,
  systemVoice: null,
  cloudVoice: "alloy",
  ratePct: 100,
  pitchPct: 100,
  wake: { enabled: false, phrase: "Jarvis", sensitivity: 50, cpuBudgetPct: 2, pauseOnBatterySaver: true },
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
};

beforeEach(async () => {
  useVoiceStore.setState({ config: CONFIG, phase: "idle", hudOpen: false, mic: [], reply: null, transcript: "" });
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

describe("voice UI", () => {
  it("mic indicator is always visible and names the purpose while the mic is on", () => {
    const { rerender } = render(<MicIndicator />);
    expect(screen.getByRole("button", { name: "Голосовые команды" })).toBeTruthy();
    act(() => useVoiceStore.setState({ mic: ["wake", "command"] }));
    rerender(<MicIndicator />);
    expect(screen.getByText("Слушаю")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Микрофон включён: Фраза активации, Слушаю" })).toBeTruthy();
  });

  it("HUD in typed mode with a result and a confirmation", () => {
    act(() => useVoiceStore.setState({ hudOpen: true }));
    render(<VoiceHud />);
    expect(screen.getByRole("dialog", { name: "Голосовые команды" })).toBeTruthy();
    expect(screen.getByPlaceholderText("Введите команду…")).toBeTruthy();
    expect(screen.getByText(/Распознавание речи выключено/)).toBeTruthy();
    act(() => useVoiceStore.setState({ phase: "confirm", confirmText: "Остановить всю работу AI в проекте Schoolify?" }));
    expect(screen.getByRole("button", { name: "Да, выполнить" })).toBeTruthy();
    act(() => useVoiceStore.setState({ phase: "result", confirmText: null, reply: { text: "Готово.", tone: "ok", outcome: "ok" } }));
    expect(screen.getByRole("status").textContent).toBe("Готово.");
  });

  it("sound & notification settings render every cue and event toggle", () => {
    render(<SoundSection projectId={null} />);
    expect(screen.getByText("Звуки интерфейса", { selector: "h3" })).toBeTruthy();
    for (const label of ["Фраза активации", "Выполнено", "Ошибка", "Подтверждение", "Развёртывание", "Тревога"]) {
      expect(screen.getByRole("button", { name: label })).toBeTruthy();
    }
    expect(screen.getByRole("switch", { name: "Сборка упала" })).toBeTruthy();
    expect(screen.getByRole("switch", { name: "Тихие часы" })).toBeTruthy();
  });

  it("Russian replies decline counts", async () => {
    expect(i18n.language).toBe("ru");
    const r = await executeIntent(
      { kind: "stopProject", project: { id: "p", name: "Schoolify", via: "spoken" }, query: "Schoolify" },
      {
        stopProject: async () => 5,
      } as unknown as Parameters<typeof executeIntent>[1],
    );
    expect(r.reply).toBe("Проект Schoolify остановлен: остановлено 5 агентов.");
  });
});
