import type { ClaudeEngineStatus } from "@orkestr/protocol";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const calls: { method: string; params: Record<string, unknown> }[] = [];
/** What the core has saved (survives a remount = reopening the page / a restart). */
let saved = "sonnet";

const status = (): ClaudeEngineStatus => ({
  installed: true,
  signedIn: true,
  subscription: "max",
  providerId: "prov-claude",
  managerOnClaude: true,
  rolesOnClaude: 18,
  reason: null,
  model: saved,
});

const fake = {
  call: vi.fn((method: string, params: Record<string, unknown>) => {
    calls.push({ method, params });
    switch (method) {
      case "claudeEngine.status":
        return Promise.resolve(status());
      case "claudeEngine.setModel":
        saved = params.model as string;
        return Promise.resolve(status());
      case "claudeEngine.useForEverything":
        saved = params.model as string;
        return Promise.resolve({ status: status(), manager: {}, routing: {} });
      default:
        return Promise.resolve({});
    }
  }),
  subscribe: vi.fn(() => () => {}),
};

vi.mock("../../core/client", () => ({ getClient: () => fake, maybeClient: () => fake }));

import { applyLanguagePreference } from "../../i18n";
import { useAppStore } from "../../store/app";
import { ClaudeEngineCard } from "./ClaudeEngineCard";

const select = () => screen.getByTestId("claude-engine-model") as HTMLSelectElement;

beforeEach(async () => {
  calls.length = 0;
  saved = "sonnet";
  useAppStore.setState({ connection: { ...useAppStore.getState().connection, state: "open" } });
  await applyLanguagePreference("en");
});
afterEach(() => cleanup());

describe("Claude Code card model", () => {
  it("saves the chosen model and shows it again after reopening", async () => {
    const first = render(<ClaudeEngineCard />);
    await waitFor(() => expect(calls.some((c) => c.method === "claudeEngine.status")).toBe(true));
    await waitFor(() => expect(select().value).toBe("sonnet"));
    fireEvent.change(select(), { target: { value: "opus" } });
    await waitFor(() => expect(calls.find((c) => c.method === "claudeEngine.setModel")?.params).toEqual({ model: "opus" }));
    expect(select().value).toBe("opus");
    first.unmount();

    // Reopen: the dropdown shows the saved Opus, not the old default.
    render(<ClaudeEngineCard />);
    await waitFor(() => expect(select().value).toBe("opus"));
  });

  it("«use for everything» applies the selected model and a refresh keeps it", async () => {
    saved = "opus";
    render(<ClaudeEngineCard />);
    await waitFor(() => expect(select().value).toBe("opus"));
    fireEvent.click(screen.getByRole("button", { name: /Claude Code for everything/i }));
    await waitFor(() => expect(calls.find((c) => c.method === "claudeEngine.useForEverything")?.params).toEqual({ model: "opus" }));
    fireEvent.click(screen.getByRole("button", { name: /check claude code again/i }));
    await waitFor(() => expect(calls.filter((c) => c.method === "claudeEngine.status").length).toBeGreaterThan(1));
    expect(select().value).toBe("opus");
  });
});
