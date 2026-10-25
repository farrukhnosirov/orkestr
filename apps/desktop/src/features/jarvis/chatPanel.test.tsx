import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../core/client", () => ({
  maybeClient: () => null,
  getClient: () => {
    throw new Error("offline");
  },
}));

import { applyLanguagePreference } from "../../i18n";
import { ChatTranscript, ConversationModeSwitch } from "./ChatPanel";
import { useJarvisStore } from "./store";

beforeEach(async () => {
  await applyLanguagePreference("ru");
  useJarvisStore.setState({ mode: "commands", entries: [], streamingId: null, lastReply: "" });
});
afterEach(() => cleanup());

describe("conversation UI", () => {
  it("switches between command and conversation mode", () => {
    render(<ConversationModeSwitch />);
    const chat = screen.getByRole("radio", { name: /Разговор/ });
    expect(chat.getAttribute("aria-checked")).toBe("false");
    fireEvent.click(chat);
    expect(useJarvisStore.getState().mode).toBe("chat");
    expect(screen.getByRole("radio", { name: /Разговор/ }).getAttribute("aria-checked")).toBe("true");
  });

  it("shows the transcript with the source of each reply and a streaming placeholder", () => {
    useJarvisStore.setState({
      entries: [
        { id: "1", role: "user", text: "привет", source: "user" },
        { id: "2", role: "assistant", text: "Здравствуйте, сэр.", source: "template" },
        { id: "3", role: "user", text: "что такое чёрная дыра", source: "user" },
        { id: "4", role: "assistant", text: "", source: "ai", streaming: true },
      ],
      streamingId: "m",
    });
    render(<ChatTranscript />);
    expect(screen.getByText("Здравствуйте, сэр.")).toBeTruthy();
    expect(screen.getByText("мгновенный ответ")).toBeTruthy();
    expect(screen.getByText("Думаю…")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Остановить ответ" })).toBeTruthy();
  });
});
