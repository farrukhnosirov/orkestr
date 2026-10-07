/**
 * Regression: "Open Terminal" issued before the Terminal panel mounts (the dock
 * mounts panel content asynchronously) must still start a shell on mount.
 */
import type { TerminalSession } from "@orkestr/protocol";
import { cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { commands } from "../../features/commands/registry";
import { useAppStore } from "../../store/app";
import TerminalPanel from "./index";
import { useTerminalUi } from "./store";

const calls: string[] = [];

function session(): TerminalSession {
  return {
    terminalId: "0190f0b0-0000-7000-8000-0000000000t9",
    projectId: null,
    owner: { kind: "user" },
    kind: "pty",
    shellId: "pwsh",
    shellKind: "PowerShell7",
    title: "PowerShell 7",
    cwd: "C:/work",
    cols: 120,
    rows: 30,
    pid: 42,
    status: "running",
    exitCode: null,
    readOnly: false,
    createdAt: "2026-10-02T00:00:00Z",
    outputBytes: 0,
  };
}

const fake = {
  state: "open",
  call: vi.fn(async (method: string) => {
    calls.push(method);
    if (method === "terminal.list") return { terminals: [] };
    if (method === "terminal.shells") return { shells: [] };
    if (method === "terminal.open") return { terminal: session() };
    return {};
  }),
  subscribe: vi.fn(() => () => undefined),
};

vi.mock("../../core/client", () => ({ getClient: () => fake, maybeClient: () => fake }));
vi.mock("../../components/workspaceActions", () => ({
  workspaceActions: { openPanel: vi.fn(), resetLayout: vi.fn() },
}));
vi.mock("./TerminalView", () => ({ TerminalView: () => null }));

beforeEach(() => {
  calls.length = 0;
  useTerminalUi.setState({ pendingOpen: false });
  useAppStore.setState((s) => ({ connection: { ...s.connection, state: "open" } }));
});

afterEach(() => cleanup());

describe("Open Terminal before the panel mounts", () => {
  it("consumes the pending request once and opens a shell", async () => {
    // The command runs first (panel not mounted yet), exactly like the dock flow.
    await commands.find((c) => c.id === "terminal.open")?.run({ projectId: null });
    expect(useTerminalUi.getState().pendingOpen).toBe(true);
    render(<TerminalPanel projectId={null} />);
    await waitFor(() => expect(calls).toContain("terminal.open"));
    expect(useTerminalUi.getState().pendingOpen).toBe(false);
    expect(calls.filter((m) => m === "terminal.open")).toHaveLength(1);
  });

  it("does not open a shell on mount without a request", async () => {
    render(<TerminalPanel projectId={null} />);
    await waitFor(() => expect(calls).toContain("terminal.list"));
    await new Promise((r) => setTimeout(r, 20));
    expect(calls).not.toContain("terminal.open");
  });

  it("the store flag is consumed exactly once", () => {
    useTerminalUi.getState().requestOpen();
    expect(useTerminalUi.getState().consumeOpenRequest()).toBe(true);
    expect(useTerminalUi.getState().consumeOpenRequest()).toBe(false);
  });
});
