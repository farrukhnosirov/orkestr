/** Agent Hub and orb menu: rendering (en / ru), actions, keyboard. */
import type { AgentInstance } from "@orkestr/protocol";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { applyLanguagePreference } from "../../i18n";
import { useSecurityStore } from "../security/store";
import { IDLE_MAIN_STATE } from "./channel";
import { agentItem, groupByProject } from "./hubModel";
import { OrbHub, type OrbHubProps } from "./OrbHub";
import { OrbMenu } from "./OrbMenu";

function agent(id: string, state: string, name: string): AgentInstance {
  return {
    id,
    templateId: "qa",
    projectId: "p1",
    taskId: null,
    name,
    state,
    stoppedAt: null,
    activity: null,
    maxTurns: 10,
    turnsUsed: 4,
    lastActivityAt: "2026-10-23T10:00:00Z",
    startedAt: null,
    createdAt: "2026-10-23T10:00:00Z",
  } as unknown as AgentInstance;
}

function props(over: Partial<OrbHubProps> = {}): OrbHubProps {
  const items = [agentItem(agent("a1", "Coding", "Backend")), agentItem(agent("a2", "Error", "Frontend"))];
  return {
    groups: groupByProject(items, [{ id: "p1", name: "Shop", color: null }]),
    items,
    loaded: true,
    voice: { ...IDLE_MAIN_STATE.voice, enabled: true },
    onPtt: vi.fn(),
    onAction: vi.fn(() => Promise.resolve()),
    onMessage: vi.fn(() => Promise.resolve()),
    onMessageManager: vi.fn(() => Promise.resolve()),
    onOpenMain: vi.fn(),
    onOpenSettings: vi.fn(),
    onOpenApprovals: vi.fn(),
    onClose: vi.fn(),
    ...over,
  };
}

beforeEach(async () => {
  await applyLanguagePreference("en");
  useSecurityStore.setState({ pending: [], flags: { ...useSecurityStore.getState().flags, allStopped: false } });
});
afterEach(cleanup);

describe("Agent Hub", () => {
  it("lists agents grouped by project with their actions", async () => {
    const p = props();
    render(<OrbHub {...p} />);
    expect(screen.getByRole("dialog", { name: "Agent Hub" })).toBeTruthy();
    expect(screen.getByText("Shop")).toBeTruthy();
    expect(screen.getByText("Backend")).toBeTruthy();
    expect(screen.getByRole("progressbar").getAttribute("aria-valuenow")).toBe("40");
    // Error row first (attention), offers restart; the working row offers stop.
    const stop = screen.getByRole("button", { name: "Stop" });
    await act(async () => fireEvent.click(stop));
    expect(p.onAction).toHaveBeenCalledWith(expect.objectContaining({ id: "a1" }), "stop");
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Restart" })));
    expect(p.onAction).toHaveBeenCalledWith(expect.objectContaining({ id: "a2" }), "restart");
  });

  it("sends a quick message to an agent", async () => {
    const p = props();
    render(<OrbHub {...p} />);
    fireEvent.click(screen.getAllByRole("button", { name: "Write a message" })[0]!);
    const field = screen.getByPlaceholderText(/via the Manager/);
    fireEvent.change(field, { target: { value: "please rerun" } });
    await act(async () => fireEvent.submit(field.closest("form")!));
    expect(p.onMessage).toHaveBeenCalledWith(expect.objectContaining({ kind: "agent" }), "please rerun");
    expect(await screen.findByText("Sent")).toBeTruthy();
  });

  it("asks before the emergency stop and closes on Escape", () => {
    const p = props();
    render(<OrbHub {...p} />);
    fireEvent.click(screen.getByRole("button", { name: "Stop all" }));
    expect(screen.getByRole("button", { name: "Confirm: stop all AI" })).toBeTruthy();
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    expect(p.onClose).toHaveBeenCalled();
  });

  it("moves between rows with the arrow keys", () => {
    render(<OrbHub {...props()} />);
    const dialog = screen.getByRole("dialog");
    fireEvent.keyDown(dialog, { key: "ArrowDown" });
    fireEvent.keyDown(dialog, { key: "ArrowDown" });
    expect(document.activeElement?.getAttribute("data-orb-row")).toBe("a1");
  });

  it("push-to-talk presses and releases", () => {
    const p = props();
    render(<OrbHub {...p} />);
    const mic = screen.getByRole("button", { name: "Hold to talk" });
    fireEvent.pointerDown(mic, { button: 0 });
    fireEvent.pointerUp(mic);
    expect(p.onPtt).toHaveBeenNthCalledWith(1, true);
    expect(p.onPtt).toHaveBeenNthCalledWith(2, false);
  });

  it("shows inline approvals and renders in Russian", async () => {
    useSecurityStore.setState({
      pending: [
        { id: "ap1", projectId: "p1", status: "Pending", risk: "Low", danger: null, actionSummary: "Run npm test", requestedAt: "2026-10-23T10:00:00Z" } as never,
      ],
    });
    await applyLanguagePreference("ru");
    render(<OrbHub {...props({ groups: [], items: [] })} />);
    expect(screen.getByRole("dialog", { name: "Хаб агентов" })).toBeTruthy();
    expect(screen.getByText("1 подтверждение")).toBeTruthy();
    expect(screen.getByText("Сейчас нет агентов")).toBeTruthy();
    expect(screen.getByRole("button", { name: /Разрешить/ })).toBeTruthy();
  });
});

describe("orb menu", () => {
  it("offers hub, animation levels and hide", () => {
    const onAction = vi.fn();
    render(<OrbMenu animation="Reduced" showBadge onAction={onAction} onClose={vi.fn()} />);
    expect(screen.getByRole("menuitemradio", { name: "Reduced" }).getAttribute("aria-checked")).toBe("true");
    fireEvent.click(screen.getByRole("menuitemradio", { name: "Still" }));
    expect(onAction).toHaveBeenCalledWith({ kind: "animation", value: "Static" });
    fireEvent.click(screen.getByRole("menuitem", { name: "Hide the orb" }));
    expect(onAction).toHaveBeenCalledWith({ kind: "hide" });
  });
});
