/**
 * Live security state across windows and against racing snapshots:
 * - a re-arm made in another window (`EmergencyFlagsChanged`) clears "stopped" here;
 * - an approvals.list answer never drops an approval requested while it was in flight,
 *   nor resurrects one resolved meanwhile.
 */
import type { Approval, EmergencyFlagsView, Event as CoreEvent } from "@orkestr/protocol";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const handlers = new Set<(ev: CoreEvent) => void>();
let listResolvers: ((r: { approvals: Approval[] }) => void)[] = [];
let statusFlags: EmergencyFlagsView = { allStopped: false, fullAutoDisabled: false, networkToolsDisabled: false, vaultLocked: false };

const fakeClient = {
  state: "open",
  call: vi.fn((method: string) => {
    if (method === "approvals.list") return new Promise((resolve) => listResolvers.push(resolve));
    if (method === "emergency.status") return Promise.resolve({ flags: statusFlags, activeAgents: 0 });
    return Promise.resolve({});
  }),
  subscribe: vi.fn((_topics: string[], h: (ev: CoreEvent) => void) => {
    handlers.add(h);
    return () => handlers.delete(h);
  }),
  onStateChange: vi.fn(() => () => undefined),
};

vi.mock("../../core/client", () => ({
  maybeClient: () => fakeClient,
  getClient: () => fakeClient,
}));

const { initSecurityLive, refreshPending, useSecurityStore } = await import("./store");
const { EMPTY_FLAGS } = await import("./logic");

function approval(id: string, requestedAt: string, status: Approval["status"] = "Pending"): Approval {
  return {
    id,
    projectId: null,
    taskId: null,
    agentId: null,
    toolCallId: null,
    capability: "Terminal",
    actionSummary: `run ${id}`,
    risk: "Medium",
    reason: "needs approval",
    status,
    requestedAt,
    expiresAt: null,
    resolvedAt: null,
    resolvedBy: null,
    resolutionNote: null,
    commandPreview: null,
    danger: null,
  };
}

function emit(kind: string, payload: unknown) {
  const ev = { seq: 1, id: "e", ts: "2026-10-07T10:00:00Z", projectId: null, taskId: null, agentId: null, nodeId: null, correlationId: null, kind, payload } as CoreEvent;
  handlers.forEach((h) => h(ev));
}

const flush = () => new Promise((r) => setTimeout(r, 0));

describe("security live state", () => {
  let dispose: () => void = () => undefined;
  beforeEach(async () => {
    listResolvers = [];
    useSecurityStore.setState({ pending: [], pendingLoaded: false, flags: EMPTY_FLAGS, toasts: [] });
    dispose = initSecurityLive();
    // Settle the initial refresh.
    listResolvers.shift()?.({ approvals: [] });
    await flush();
  });
  afterEach(() => {
    dispose();
    handlers.clear();
  });

  it("follows a re-arm made in another window", () => {
    useSecurityStore.getState().setFlags({ ...EMPTY_FLAGS, allStopped: true });
    emit("EmergencyFlagsChanged", { flags: { ...EMPTY_FLAGS, allStopped: false } });
    expect(useSecurityStore.getState().flags.allStopped).toBe(false);
  });

  it("re-reads the flags when the window regains focus", async () => {
    useSecurityStore.getState().setFlags({ ...EMPTY_FLAGS, allStopped: true });
    statusFlags = { ...EMPTY_FLAGS };
    window.dispatchEvent(new Event("focus"));
    await flush();
    expect(useSecurityStore.getState().flags.allStopped).toBe(false);
  });

  it("keeps an approval requested while the list call was in flight", async () => {
    const done = refreshPending();
    const b = approval("b", "2026-10-07T10:00:01Z");
    emit("ApprovalRequested", { approval: b });
    // The snapshot was taken before B existed.
    listResolvers.shift()?.({ approvals: [] });
    await done;
    expect(useSecurityStore.getState().pending.map((a) => a.id)).toEqual(["b"]);
  });

  it("does not resurrect an approval resolved while the list call was in flight", async () => {
    const a = approval("a", "2026-10-07T10:00:00Z");
    useSecurityStore.getState().setPending([a]);
    const done = refreshPending();
    emit("ApprovalResolved", { approvalId: "a" });
    listResolvers.shift()?.({ approvals: [a] }); // stale snapshot
    await done;
    expect(useSecurityStore.getState().pending).toEqual([]);
  });

  it("drops the answer of a superseded refresh", async () => {
    const first = refreshPending();
    const second = refreshPending();
    const [r1, r2] = listResolvers;
    listResolvers = [];
    r2?.({ approvals: [approval("new", "2026-10-07T10:00:02Z")] });
    await second;
    r1?.({ approvals: [approval("old", "2026-10-07T10:00:00Z")] });
    await first;
    expect(useSecurityStore.getState().pending.map((x) => x.id)).toEqual(["new"]);
  });
});
