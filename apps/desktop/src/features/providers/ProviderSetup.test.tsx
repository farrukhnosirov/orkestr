import type { ProviderKindInfo, ProviderView } from "@orkestr/protocol";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { i18n } from "../../i18n";
import { useAppStore } from "../../store/app";
import { ProviderSetup } from "./ProviderSetup";
import { useProvidersStore } from "./store";

function kind(k: ProviderKindInfo["kind"], extra: Partial<ProviderKindInfo> = {}): ProviderKindInfo {
  return {
    kind: k,
    displayName: k === "Mock" ? "Mock (offline demo)" : k,
    description: `${k} description`,
    authMethod: "ApiKey",
    requiresApiKey: false,
    apiKeyOptional: false,
    supportsBaseUrl: false,
    defaultBaseUrl: null,
    supportsCustomHeaders: false,
    isLocal: false,
    codingAgent: false,
    binary: null,
    detected: null,
    signInNote: null,
    loginInstructions: null,
    defaultModel: null,
    ...extra,
  };
}

const mock: ProviderView = {
  provider: {
    id: "0199a213-81c0-7800-8aa1-bbab2a035a53",
    kind: "Mock",
    name: "Mock (offline demo)",
    enabled: true,
    baseUrl: null,
    authMethod: "None",
    credentialRefId: null,
    secretRefId: null,
    isLocal: true,
    defaultModel: "mock-smart",
    settings: {},
    createdAt: "2026-10-01T00:00:00Z",
    updatedAt: "2026-10-01T00:00:00Z",
  },
  status: {
    health: "Healthy",
    authState: "Authenticated",
    authDetail: "no credentials needed (offline demo)",
    healthDetail: null,
    latencyMs: 0,
    avgLatencyMs: null,
    version: null,
    checkedAt: null,
    circuitOpen: false,
    rateLimitedUntil: null,
    quotaExhaustedUntil: null,
  },
  hasCredential: false,
  capabilities: { tools: true, vision: false, structuredOutput: true, streaming: true, reasoning: false, codingAgent: false },
  maxContextTokens: 128000,
  locality: "Local",
  costModel: "Local",
  codingAgent: false,
};

afterEach(() => {
  cleanup();
  useProvidersStore.setState({ kinds: [], providers: [], loaded: false, loading: false, error: null });
});

describe("<ProviderSetup/>", () => {
  it("shows a connecting state before the core is reachable", () => {
    useAppStore.getState().setConnection({ state: "connecting" });
    render(<ProviderSetup />);
    expect(screen.getByText(/Connecting to the Orkestr core/)).toBeTruthy();
  });

  it("renders kind cards, instances, CLI detection and the add form", () => {
    useAppStore.getState().setConnection({ state: "open" });
    useProvidersStore.getState().setAll(
      [
        kind("OpenAi", { requiresApiKey: true, signInNote: "Use an API key." }),
        kind("ClaudeCode", { authMethod: "CliSession", codingAgent: true, binary: "claude", detected: false, loginInstructions: "Run claude and /login." }),
        kind("Mock", { authMethod: "None", isLocal: true }),
      ],
      [mock],
      "2026-10-01",
    );
    render(<ProviderSetup />);
    expect(screen.getByRole("heading", { name: "OpenAi" })).toBeTruthy();
    expect(screen.getByText("Not installed")).toBeTruthy();
    expect(screen.getByText(/Sign in via the ClaudeCode CLI/)).toBeTruthy();
    expect(screen.getByText("Connected")).toBeTruthy();
    expect(screen.getAllByText("Mock (offline demo)").length).toBeGreaterThan(0);
    // CLI not installed → connect disabled.
    expect((screen.getByRole("button", { name: "Connect ClaudeCode" }) as HTMLButtonElement).disabled).toBe(true);
    // API key kinds require a key before submitting.
    fireEvent.click(screen.getByRole("button", { name: "Add OpenAi" }));
    fireEvent.click(screen.getByRole("button", { name: "Add provider" }));
    expect(screen.getByRole("alert").textContent).toMatch(/API key/);
    // Only the requested kinds are shown when filtered (onboarding).
    cleanup();
    render(<ProviderSetup kinds={["Mock"]} compact />);
    expect(screen.queryByRole("heading", { name: "OpenAi" })).toBeNull();
  });
});

describe("<ProviderSetup/> in Russian", () => {
  afterEach(async () => {
    await act(async () => {
      await i18n.changeLanguage("en");
    });
  });

  it("translates kind cards, badges, statuses and auth details", async () => {
    await act(async () => {
      await i18n.changeLanguage("ru");
    });
    useAppStore.getState().setConnection({ state: "open" });
    useProvidersStore.getState().setAll(
      [
        kind("ClaudeCode", { authMethod: "CliSession", codingAgent: true, binary: "claude", detected: false, loginInstructions: "Run claude and /login." }),
        kind("Mock", { authMethod: "None", isLocal: true }),
      ],
      [mock],
      "2026-10-01",
    );
    render(<ProviderSetup />);
    expect(screen.getByRole("heading", { name: "Mock (офлайн-демо)" })).toBeTruthy();
    expect(screen.getAllByText("Не установлен").length).toBeGreaterThan(0);
    expect(screen.getByText("Вход через CLI ClaudeCode:")).toBeTruthy();
    expect(screen.getAllByText("Подключён").length).toBeGreaterThan(0);
    expect(screen.getByText("учётные данные не нужны (офлайн-демо)")).toBeTruthy();
    expect((screen.getByRole("button", { name: "Подключить ClaudeCode" }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByRole("button", { name: "Проверить подключение" })).toBeTruthy();
  });
});
