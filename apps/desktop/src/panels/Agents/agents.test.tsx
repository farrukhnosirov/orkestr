import type { AgentInstance, AgentTemplate, AgentToolCallView, Event } from "@orkestr/protocol";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { useAppStore } from "../../store/app";
import {
  EMPTY_TRANSCRIPT,
  applyAgentEvent,
  applyTranscriptEvent,
  blankDraft,
  duplicateDraft,
  elapsed,
  formatCost,
  formatTokens,
  mergeTranscript,
  roleInitials,
  timeline,
  validateDraft,
} from "./model";
import { useAgentsUi } from "./store";

const PROJECT = "0190f0b0-0000-7000-8000-ffffffffffff";
const AGENT = "0190f0b0-0000-7000-8000-00000000000a";
const TASK = "0190f0b0-0000-7000-8000-00000000000b";
const CALL = "0190f0b0-0000-7000-8000-00000000000c";

function agent(patch: Partial<AgentInstance> = {}): AgentInstance {
  return {
    id: AGENT,
    templateId: null,
    projectId: PROJECT,
    taskId: TASK,
    teamId: null,
    parentAgentId: null,
    name: "Backend Engineer #1",
    role: "Backend",
    state: "Thinking",
    model: { providerId: "p", model: "mock-smart" },
    worktree: { branch: "agent/backend-1234abcd", path: "/wt" },
    usage: { inputTokens: 1200, outputTokens: 300, costUsd: 0.25 },
    turnsUsed: 2,
    lastError: null,
    createdAt: "2026-10-02T10:00:00Z",
    startedAt: "2026-10-02T10:00:00Z",
    stoppedAt: null,
    lastActivityAt: null,
    templateVersion: "1.0.0",
    worktreePolicy: "DedicatedWorktree",
    maxTurns: 40,
    budget: null,
    executionId: null,
    executionStyle: "chat",
    cliSession: null,
    activity: "Waiting for the model",
    ...patch,
  };
}

function ev(kind: string, payload: unknown): Event {
  return {
    seq: 1,
    id: "e",
    ts: "",
    projectId: PROJECT,
    taskId: null,
    agentId: AGENT,
    nodeId: null,
    correlationId: null,
    kind,
    payload,
  } as Event;
}

const toolCall: AgentToolCallView = {
  toolCall: {
    id: CALL,
    executionId: "x",
    agentId: AGENT,
    taskId: TASK,
    tool: "write_file",
    capability: "Write",
    input: { path: "src/a.rs" },
    outputSummary: null,
    status: "Running",
    approvalId: null,
    startedAt: "2026-10-02T10:00:01Z",
    finishedAt: null,
  },
  result: null,
  durationMs: null,
};

describe("agents model", () => {
  it("applies list events", () => {
    const a = agent();
    let list = applyAgentEvent([], ev("AgentSpawned", { agent: a }));
    expect(list).toHaveLength(1);
    list = applyAgentEvent(list, ev("AgentStateChanged", { agentId: AGENT, from: "Thinking", to: "Coding", activity: "write_file src/a.rs" }));
    expect(list[0]?.state).toBe("Coding");
    expect(list[0]?.activity).toBe("write_file src/a.rs");
    list = applyAgentEvent(list, ev("AgentUpdated", { agent: { ...a, turnsUsed: 5 } }));
    expect(list[0]?.turnsUsed).toBe(5);
    expect(applyAgentEvent(list, ev("TaskCreated", {}))).toBe(list);
  });

  it("streams deltas, tool calls and merges persisted messages", () => {
    let t = applyTranscriptEvent(EMPTY_TRANSCRIPT, ev("AgentMessage", { agentId: AGENT, messageId: "m1", role: "Assistant", delta: "Hel", done: false }));
    t = applyTranscriptEvent(t, ev("AgentMessage", { agentId: AGENT, messageId: "m1", role: "Assistant", delta: "lo", done: false }));
    expect(t.streaming.m1?.text).toBe("Hello");
    t = applyTranscriptEvent(t, ev("ToolCallStarted", { toolCall: toolCall.toolCall }));
    t = applyTranscriptEvent(t, ev("ToolCallFinished", { toolCallId: CALL, status: "Succeeded", outputSummary: "created src/a.rs", durationMs: 12 }));
    expect(t.toolCalls[0]?.toolCall.status).toBe("Succeeded");
    expect(t.toolCalls[0]?.durationMs).toBe(12);
    const items = timeline(t);
    expect(items.map((i) => i.kind)).toEqual(["tool", "message"]);
    t = mergeTranscript(
      t,
      [{ id: "m1", seq: 1, agentId: AGENT, executionId: null, role: "Assistant", text: "Hello", toolCallIds: [CALL], createdAt: "" }],
      [{ ...toolCall, toolCall: { ...toolCall.toolCall, status: "Succeeded" }, result: "ok" }],
    );
    expect(t.streaming).toEqual({});
    const merged = timeline(t);
    expect(merged.map((i) => i.kind)).toEqual(["message", "tool"]);
  });

  it("formats and validates", () => {
    expect(formatTokens(950)).toBe("950");
    expect(formatTokens(1500)).toBe("1.5k");
    expect(formatTokens(2_500_000)).toBe("2.5M");
    expect(formatCost(0)).toBe("$0");
    expect(formatCost(0.004)).toBe("<$0.01");
    expect(formatCost(1.5)).toBe("$1.50");
    expect(elapsed({ startedAt: "2026-10-02T10:00:00Z", stoppedAt: "2026-10-02T10:02:05Z", createdAt: "" }, 0)).toBe("2m 05s");
    expect(roleInitials("UiUx")).toBe("UU");
    expect(roleInitials("Backend")).toBe("BA");
    expect(roleInitials({ Custom: "Payroll Expert" })).toBe("PE");
    expect(validateDraft(blankDraft())).toBeNull();
    expect(validateDraft({ ...blankDraft(), maxTurns: 0 })).toMatch(/turns/);
    expect(validateDraft({ ...blankDraft(), version: "v1" })).toMatch(/Version/);
    expect(validateDraft({ ...blankDraft(), name: " " })).toMatch(/Name/);
  });

  it("duplicates templates as a fresh custom draft", () => {
    const t = { ...blankDraft(), id: "t", source: "Builtin", createdAt: "", updatedAt: "", name: "Backend Engineer", version: "2.3.1" } as AgentTemplate;
    const d = duplicateDraft(t);
    expect(d.name).toBe("Backend Engineer (copy)");
    expect(d.version).toBe("1.0.0");
    expect("id" in d).toBe(false);
  });
});

// ── panel ───────────────────────────────────────────────

let handlers: Array<(ev: Event) => void> = [];
const calls: Array<{ method: string; params: unknown }> = [];
const TEMPLATE: AgentTemplate = {
  ...blankDraft(),
  id: "0190f0b0-0000-7000-8000-0000000000a2",
  name: "Backend Engineer",
  source: "Builtin",
  createdAt: "",
  updatedAt: "",
};

const fakeClient = {
  call: vi.fn(async (method: string, params: unknown) => {
    calls.push({ method, params });
    switch (method) {
      case "agents.list":
        return { agents: [agent()] };
      case "agents.transcript":
        return {
          agent: agent(),
          messages: [{ id: "m0", seq: 1, agentId: AGENT, executionId: null, role: "Assistant", text: "Exploring the code", toolCallIds: [CALL], createdAt: "" }],
          toolCalls: [{ ...toolCall, toolCall: { ...toolCall.toolCall, status: "Succeeded", outputSummary: "created src/a.rs" }, result: "created src/a.rs (3 bytes)" }],
        };
      case "templates.list":
        return { templates: [TEMPLATE] };
      case "tasks.list":
        return { tasks: [] };
      case "tasks.graph":
        return { nodes: [], edges: [], ready: [], topoOrder: [], criticalPath: [], criticalPathLength: 0 };
      case "providers.list":
        return { providers: [], kinds: [], pricingTableDate: "" };
      case "providers.kinds":
        return { kinds: [] };
      case "agents.stop":
        return { agent: agent({ state: "Stopped" }) };
      case "agents.spawn":
        return { agent: agent({ id: "new" }), task: { title: "Do it" } };
      default:
        throw new Error(`unexpected ${method}`);
    }
  }),
  subscribe: vi.fn((_topics: string[], h: (ev: Event) => void) => {
    handlers.push(h);
    return () => {
      handlers = handlers.filter((x) => x !== h);
    };
  }),
};

vi.mock("../../core/client", () => ({
  maybeClient: () => fakeClient,
  getClient: () => fakeClient,
}));

const { default: AgentsPanel } = await import("./index");

describe("AgentsPanel", () => {
  beforeEach(() => {
    calls.length = 0;
    useAppStore.getState().setConnection({ state: "open" });
    useAgentsUi.setState({ tab: "agents", selectedAgentId: null, spawnOpen: false, editor: null, importOpen: false, toasts: [] });
  });
  afterEach(() => cleanup());

  it("shows the empty state without a project", () => {
    render(<AgentsPanel projectId={null} />);
    expect(screen.getByText("No project open")).toBeTruthy();
  });

  it("lists agents, shows the live transcript and stops an agent", async () => {
    render(<AgentsPanel projectId={PROJECT} />);
    await screen.findByRole("region", { name: "Agent Backend Engineer #1" });
    expect(screen.getAllByText("agent/backend-1234abcd").length).toBeGreaterThan(0);
    await screen.findByText("Exploring the code");
    expect(screen.getByText("write_file")).toBeTruthy();
    expect(useAgentsUi.getState().activeAgentIds).toEqual([AGENT]);

    // Streaming delta appears live.
    act(() => {
      for (const h of handlers) h(ev("AgentMessage", { agentId: AGENT, messageId: "m9", role: "Assistant", delta: "Now testing…", done: false }));
    });
    await screen.findByText("Now testing…");

    fireEvent.click(screen.getByRole("button", { name: "Stop" }));
    await waitFor(() => expect(calls.some((c) => c.method === "agents.stop")).toBe(true));
    expect(calls.find((c) => c.method === "agents.stop")?.params).toEqual({ agentId: AGENT });
  });

  it("spawns an agent on a new task", async () => {
    render(<AgentsPanel projectId={PROJECT} />);
    await screen.findByRole("region", { name: "Agent Backend Engineer #1" });
    act(() => useAgentsUi.getState().openSpawn());
    const dialog = await screen.findByRole("dialog", { name: "Spawn agent" });
    expect(dialog).toBeTruthy();
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Add CSV export" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Spawn agent" }));
    await waitFor(() => expect(calls.some((c) => c.method === "agents.spawn")).toBe(true));
    expect(calls.find((c) => c.method === "agents.spawn")?.params).toEqual({
      projectId: PROJECT,
      templateId: TEMPLATE.id,
      newTask: { title: "Add CSV export" },
    });
  });

  it("shows templates with official badges", async () => {
    useAgentsUi.setState({ tab: "templates" });
    render(<AgentsPanel projectId={PROJECT} />);
    await screen.findByText("Backend Engineer");
    expect(screen.getByText("Official")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Delete template" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Duplicate template" }));
    await screen.findByRole("dialog", { name: "New template" });
    expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe("Backend Engineer (copy)");
  });
});
