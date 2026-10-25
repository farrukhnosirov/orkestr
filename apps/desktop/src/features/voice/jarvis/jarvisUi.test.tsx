/** Jarvis voice card: generic props, ru / en strings, actions, the orb popup mapping. */
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { applyLanguagePreference } from "../../../i18n";
import { OrbVoicePopup } from "../../orb/OrbVoicePopup";
import { JarvisVoice } from "./JarvisVoice";

beforeEach(async () => {
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

const base = { level: 0.4, partialText: "", finalText: "", replyText: "", fps: 0, motion: false } as const;

describe("JarvisVoice", () => {
  it("listening: live words, status, mode chip, mic indicator — never empty", () => {
    const { rerender } = render(<JarvisVoice {...base} state="listening" micOpen engine="Vosk" />);
    expect(screen.getByRole("dialog", { name: "Голосовой помощник" })).toBeTruthy();
    expect(screen.getByText("Слушаю…")).toBeTruthy();
    expect(screen.getByText("Команда")).toBeTruthy();
    expect(screen.getByText("Vosk")).toBeTruthy();
    expect(screen.getByLabelText("Микрофон включён")).toBeTruthy();
    // Nothing heard yet: a hint instead of an empty box.
    expect(screen.getByText(/Говорите/)).toBeTruthy();
    rerender(<JarvisVoice {...base} state="listening" partialText="открой ютуб" />);
    expect(document.querySelectorAll(".jv-word")).toHaveLength(2);
    expect(document.querySelector(".jv__heard--live")?.textContent).toBe("открой ютуб");
  });

  it("thinking → speaking → done with the final transcript and reply", () => {
    const { rerender } = render(<JarvisVoice {...base} state="thinking" finalText="открой ютуб" />);
    expect(screen.getByText("Думаю…")).toBeTruthy();
    expect(document.querySelector(".jv__heard--final")?.textContent).toBe("открой ютуб");
    rerender(<JarvisVoice {...base} state="speaking" finalText="открой ютуб" replyText="Открываю YouTube, сэр." speaking />);
    expect(screen.getByText("Отвечаю…")).toBeTruthy();
    expect(screen.getByRole("status", { name: "Открываю YouTube, сэр." })).toBeTruthy();
    rerender(<JarvisVoice {...base} state="done" finalText="открой ютуб" replyText="Открываю YouTube, сэр." />);
    expect(document.querySelector("[data-jarvis-state='done']")).toBeTruthy();
  });

  it("quick actions call back; confirm shows yes / no; nothing takes the focus", () => {
    const onRepeat = vi.fn();
    const onCancel = vi.fn();
    const onOpen = vi.fn();
    const onConfirm = vi.fn();
    const before = document.activeElement;
    const { rerender } = render(<JarvisVoice {...base} state="done" replyText="Готово, сэр." onRepeat={onRepeat} onCancel={onCancel} onOpen={onOpen} />);
    expect(document.activeElement).toBe(before);
    fireEvent.click(screen.getByRole("button", { name: /Повторить/ }));
    fireEvent.click(screen.getByRole("button", { name: "Отмена" }));
    fireEvent.click(screen.getByRole("button", { name: /Открыть в Orkestr/ }));
    expect(onRepeat).toHaveBeenCalledOnce();
    expect(onCancel).toHaveBeenCalledOnce();
    expect(onOpen).toHaveBeenCalledOnce();
    rerender(<JarvisVoice {...base} state="listening" phase="confirm" confirmText="Закрыть Chrome?" onConfirm={onConfirm} />);
    expect(screen.getByText("Жду вашего ответа")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /Да/ }));
    fireEvent.click(screen.getByRole("button", { name: "Нет" }));
    expect(onConfirm.mock.calls).toEqual([[true], [false]]);
  });

  it("modes for other packages: chat and cursor", () => {
    const { rerender } = render(<JarvisVoice {...base} state="listening" mode="chat" />);
    expect(screen.getByText("Разговор")).toBeTruthy();
    rerender(<JarvisVoice {...base} state="listening" mode="cursor" />);
    expect(screen.getByText("Курсор")).toBeTruthy();
    expect(document.querySelector(".jv--mode-cursor")).toBeTruthy();
  });

  it("English strings", async () => {
    await act(async () => {
      await applyLanguagePreference("en");
    });
    render(<JarvisVoice {...base} state="error" replyText="Microphone unavailable" replyTone="error" />);
    expect(screen.getByText("Something went wrong")).toBeTruthy();
  });
});

describe("OrbVoicePopup", () => {
  const common = {
    onOpenMain: () => undefined,
    size: 150,
    fps: 0,
    motion: false,
    signal: null,
    themeKey: "t",
    accent: null,
    leaving: false,
  } as const;

  it("maps the main window's voice state onto the card (partial while listening, final after)", () => {
    const onAction = vi.fn();
    const voice = { enabled: true, phase: "listening", speaking: false, level: 0.5, micOpen: true, transcript: "прокрути вниз", sttEngine: "Vosk", ttsEngine: "Silero", source: "WakeWord" };
    const { rerender } = render(<OrbVoicePopup {...common} voice={voice} state="listening" side="right" onAction={onAction} />);
    expect(document.querySelector(".jv__heard--live")?.textContent).toBe("прокрути вниз");
    expect(screen.getByText("Vosk (офлайн)")).toBeTruthy();
    expect(document.querySelector(".jv--reverse")).toBeNull();
    rerender(
      <OrbVoicePopup
        {...common}
        voice={{ ...voice, phase: "result", speaking: true, micOpen: false, reply: { text: "Прокручиваю вниз, босс.", tone: "ok" } }}
        state="speaking"
        side="left"
        onAction={onAction}
      />,
    );
    expect(document.querySelector(".jv__heard--final")?.textContent).toBe("прокрути вниз");
    expect(screen.getByRole("status", { name: "Прокручиваю вниз, босс." })).toBeTruthy();
    expect(screen.getByText("Silero (офлайн)")).toBeTruthy();
    expect(document.querySelector(".jv--reverse")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /Повторить/ }));
    fireEvent.click(screen.getByRole("button", { name: "Отмена" }));
    expect(onAction.mock.calls).toEqual([["repeat"], ["cancel"]]);
  });
});
