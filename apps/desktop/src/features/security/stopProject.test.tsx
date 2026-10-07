/** Stop Project (§52) is reachable without voice: emergency dialog, project rail menu and command palette. */
import type { Project } from "@orkestr/protocol";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const calls: { method: string; params: unknown }[] = [];
const fakeClient = {
  state: "open",
  call: vi.fn((method: string, params: unknown) => {
    calls.push({ method, params });
    if (method === "approvals.list") return Promise.resolve({ approvals: [] });
    if (method === "emergency.stopProject" || method === "emergency.stopAll") {
      return Promise.resolve({
        agentsStopped: 2,
        otherStopped: 1,
        approvalsCancelled: 1,
        flags: { allStopped: method === "emergency.stopAll", fullAutoDisabled: false, networkToolsDisabled: false, vaultLocked: false },
      });
    }
    return Promise.resolve({});
  }),
  subscribe: vi.fn(() => () => undefined),
  onStateChange: vi.fn(() => () => undefined),
};

vi.mock("../../core/client", () => ({
  maybeClient: () => fakeClient,
  getClient: () => fakeClient,
  openProject: vi.fn(),
}));

const { i18n } = await import("../../i18n");
const { useAppStore } = await import("../../store/app");
const { useSecurityStore } = await import("./store");
const { EMPTY_FLAGS } = await import("./logic");
const { EmergencyConfirmDialog } = await import("./EmergencyStop");
const { coreCommands } = await import("../commands/coreCommands");

const P1 = "0190f0b0-0000-7000-8000-000000000001";
const P2 = "0190f0b0-0000-7000-8000-000000000002";

function project(id: string, name: string): Project {
  return { id, name } as Project;
}

describe("Stop Project", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    calls.length = 0;
    useAppStore.setState({ projects: [project(P1, "Payroll"), project(P2, "Website")], activeProjectId: P1 });
    useSecurityStore.setState({ confirmStopOpen: false, stopProjectId: null, flags: EMPTY_FLAGS, toasts: [] });
  });
  afterEach(() => cleanup());

  it("is offered for the active project next to Stop All", async () => {
    act(() => useSecurityStore.getState().setConfirmStopOpen(true));
    render(<EmergencyConfirmDialog />);
    fireEvent.click(screen.getByRole("button", { name: /Stop only “Payroll”/ }));
    await act(async () => undefined);
    expect(calls.find((c) => c.method === "emergency.stopProject")?.params).toEqual({ projectId: P1 });
    expect(calls.some((c) => c.method === "emergency.stopAll")).toBe(false);
    expect(useSecurityStore.getState().confirmStopOpen).toBe(false);
    expect(useSecurityStore.getState().toasts.at(-1)?.title).toBe("AI in “Payroll” stopped");
  });

  it("confirms a project picked from the rail, not the active one", async () => {
    act(() => useSecurityStore.getState().openStopProject(P2));
    render(<EmergencyConfirmDialog />);
    expect(screen.getByRole("heading", { name: "Stop AI in “Website”?" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /^Stop project/ }));
    await act(async () => undefined);
    expect(calls.find((c) => c.method === "emergency.stopProject")?.params).toEqual({ projectId: P2 });
  });

  it("has a palette command scoped to the current project", () => {
    const cmd = coreCommands.find((c) => c.id === "ai.stopProject");
    expect(cmd).toBeTruthy();
    expect(cmd?.enabled?.({ projectId: null })).toBe(false);
    void cmd?.run({ projectId: P1 });
    expect(useSecurityStore.getState().confirmStopOpen).toBe(true);
    expect(useSecurityStore.getState().stopProjectId).toBe(P1);
  });

  it("is translated in Russian", async () => {
    await i18n.changeLanguage("ru");
    act(() => useSecurityStore.getState().openStopProject(P2));
    render(<EmergencyConfirmDialog />);
    expect(screen.getByRole("heading", { name: "Остановить работу AI в проекте «Website»?" })).toBeTruthy();
    await i18n.changeLanguage("en");
  });
});
