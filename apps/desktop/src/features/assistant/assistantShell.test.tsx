/**
 * Assistant-mode shell (ADR-0035): the window shows only the slim assistant
 * title bar + the assistant card; the Orchestra shell (title bar, project
 * rail, dock) stays mounted but hidden so switching back restores it.
 */
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const base = {
  call: vi.fn((method: string) => {
    if (method === "computer.history") return Promise.resolve({ entries: [] });
    return Promise.resolve({});
  }),
};
/** Any other client method (subscribe / on / onStateChange …) is a no-op returning an unsubscribe. */
const fake = new Proxy(base as Record<string, unknown>, {
  get: (t, k) => (k in t ? t[k as string] : k === "then" ? undefined : () => () => {}),
});

const winCalls: string[] = [];
let native = false;

vi.mock("../../core/client", async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
  bootstrapCore: () => Promise.resolve(),
  getClient: () => fake,
  maybeClient: () => fake,
}));
vi.mock("../../lib/tauri", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../lib/tauri")>();
  return {
    ...orig,
    isTauri: () => native,
    windowControls: {
      ...orig.windowControls,
      minimize: async () => void winCalls.push("minimize"),
      close: async () => void winCalls.push("close"),
      isMaximized: async () => false,
      onResized: async () => () => {},
    },
  };
});
// The rest of the shell is stubbed: this test is about which shell renders.
const { stub } = vi.hoisted(() => ({
  stub: (name: string) => async (importOriginal: () => Promise<Record<string, unknown>>) => ({ ...(await importOriginal()), [name]: () => null }),
}));
vi.mock("../../components/TitleBar", () => ({ TitleBar: () => <header className="titlebar" /> }));
vi.mock("../../components/StatusBar", () => ({ StatusBar: () => null }));
vi.mock("../../components/CommandPalette", stub("CommandPalette"));
vi.mock("../../components/CreateProjectDialog", stub("CreateProjectDialog"));
vi.mock("../../features/theme/ThemeEditor", stub("ThemeEditor"));
vi.mock("../../features/settings/SettingsView", stub("SettingsView"));
vi.mock("../../features/onboarding/OnboardingWizard", stub("OnboardingWizard"));
vi.mock("../../features/layout/LayoutEditor", stub("LayoutEditorPanel"));
vi.mock("../../features/layout/LayoutDialogs", stub("LayoutDialogs"));
vi.mock("../../features/workbench/WorkbenchDialogs", stub("WorkbenchDialogs"));
vi.mock("../../features/workbench/WelcomePage", stub("WelcomePage"));
vi.mock("../../panels/MissionControl", stub("MissionControlOverlay"));
vi.mock("../../features/orb/register", stub("OrbHost"));
vi.mock("../../features/aiui/AiUiLayer", stub("AiUiLayer"));
vi.mock("../../features/security/SecurityLayer", stub("SecurityLayer"));
vi.mock("../../features/resume/ResumeCard", stub("ResumeCard"));
vi.mock("../../features/vscode/ExtensionLauncher", async (io: () => Promise<Record<string, unknown>>) => ({ ...(await io()), LauncherConfirmDialog: () => null, LauncherToast: () => null }));
vi.mock("../../components/ProjectRail", () => ({ ProjectRail: () => <nav data-testid="rail" /> }));
vi.mock("../../components/Workspace", () => ({ Workspace: () => <div data-testid="dock" /> }));

import { App } from "../../App";
import { applyLanguagePreference } from "../../i18n";
import { AssistantTitleBar } from "./AssistantTitleBar";
import { useAssistantStore } from "./store";

afterEach(() => {
  cleanup();
  winCalls.length = 0;
  native = false;
});

describe("assistant shell", () => {
  it.each(["en", "ru"] as const)("compact title bar has its controls (%s)", async (lang) => {
    await applyLanguagePreference(lang);
    useAssistantStore.setState({ mode: "assistant" });
    native = true;
    render(<AssistantTitleBar />);
    const bar = screen.getByTestId("assistant-titlebar");
    expect(bar.hasAttribute("data-tauri-drag-region")).toBe(true);
    expect(bar.textContent).toContain(lang === "ru" ? "Ассистент" : "Assistant");
    const ru = lang === "ru";
    expect(screen.getByRole("button", { name: ru ? "Экстренная остановка всей работы AI" : /stop/i })).toBeTruthy();
    expect(screen.getByRole("button", { name: ru ? "Настройки (Ctrl+,)" : /settings/i })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: ru ? "Свернуть" : "Minimize" }));
    fireEvent.click(screen.getByRole("button", { name: ru ? "Закрыть" : "Close" }));
    await waitFor(() => expect(winCalls).toEqual(["minimize", "close"]));
    fireEvent.click(screen.getByRole("button", { name: ru ? "Режим оркестра" : "Orchestra mode" }));
    expect(useAssistantStore.getState().mode).toBe("orchestra");
  });

  it("assistant mode renders only the assistant shell; the orchestra shell stays mounted but hidden", async () => {
    await applyLanguagePreference("en");
    useAssistantStore.setState({ mode: "assistant" });
    const { container } = render(<App />);
    await waitFor(() => expect(screen.getByTestId("assistant-surface")).toBeTruthy());
    expect(screen.getByTestId("assistant-titlebar")).toBeTruthy();
    expect(container.querySelector(".titlebar")).toBeNull();
    expect(container.querySelector(".app--assistant")).toBeTruthy();
    const body = container.querySelector(".app__body") as HTMLElement;
    expect(body.classList.contains("app__body--hidden")).toBe(true);
    expect(body.getAttribute("aria-hidden")).toBe("true");
    // rail + dock are kept (layout state) inside the hidden body only
    expect(body.contains(screen.getByTestId("rail"))).toBe(true);
    expect(body.contains(screen.getByTestId("dock"))).toBe(true);
    expect(screen.getByRole("button", { name: "Settings (Ctrl+,)" })).toBeTruthy();

    await act(async () => useAssistantStore.getState().setMode("orchestra"));
    await waitFor(() => expect(container.querySelector(".titlebar")).toBeTruthy());
    expect(screen.queryByTestId("assistant-titlebar")).toBeNull();
    expect(container.querySelector(".app__body--hidden")).toBeNull();
    expect(screen.getByTestId("dock")).toBeTruthy();
  });
});
