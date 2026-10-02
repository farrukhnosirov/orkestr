/**
 * Pure helpers of the Agents panel (unit-tested): state presentation, event
 * reducers for the agent list and the live transcript, formatting and
 * template drafts.
 */
import type {
  AgentInstance,
  AgentRole,
  AgentState,
  AgentTemplate,
  AgentToolCallView,
  AgentTranscriptMessage,
  Event,
  TemplateDraft,
  ToolCallStatus,
} from "@orkestr/protocol";
import type { Tone } from "@orkestr/ui";

export const AGENT_STATES: readonly AgentState[] = [
  "Idle",
  "Listening",
  "Thinking",
  "Coding",
  "Testing",
  "Reviewing",
  "Waiting",
  "Error",
  "Completed",
  "Stopped",
];

export const STATE_TONE: Record<AgentState, Tone> = {
  Idle: "neutral",
  Listening: "info",
  Thinking: "violet",
  Coding: "accent",
  Testing: "info",
  Reviewing: "violet",
  Waiting: "warning",
  Error: "danger",
  Completed: "success",
  Stopped: "neutral",
};

/** States in which the agent's run is still going. */
export function isActive(state: AgentState): boolean {
  return !(state === "Completed" || state === "Error" || state === "Stopped");
}

/** States that animate (busy). */
export function isBusy(state: AgentState): boolean {
  return state === "Thinking" || state === "Coding" || state === "Testing" || state === "Reviewing" || state === "Listening";
}

const ROLE_LABEL: Record<string, string> = {
  Manager: "Manager",
  Architect: "Architect",
  Backend: "Backend",
  Frontend: "Frontend",
  Database: "Database",
  Qa: "QA",
  Security: "Security",
  DevOps: "DevOps",
  UiUx: "UI/UX",
  ProductManager: "Product Manager",
  Reviewer: "Reviewer",
  Researcher: "Researcher",
};

export const BUILTIN_ROLES: readonly Exclude<AgentRole, { Custom: string }>[] = [
  "Architect",
  "Backend",
  "Frontend",
  "Database",
  "Qa",
  "Security",
  "DevOps",
  "UiUx",
  "ProductManager",
  "Reviewer",
  "Researcher",
];

export function roleLabel(role: AgentRole): string {
  if (typeof role === "object") return role.Custom;
  return ROLE_LABEL[role] ?? role;
}

/** Two-letter monogram for the role avatar. */
export function roleInitials(role: AgentRole): string {
  const label = roleLabel(role).replace(/[^A-Za-z/ ]/g, "");
  const parts = label.split(/[ /]+/).filter(Boolean);
  if (parts.length >= 2) return `${parts[0]?.[0] ?? ""}${parts[1]?.[0] ?? ""}`.toUpperCase();
  return label.slice(0, 2).toUpperCase();
}

/** Inserts or replaces an agent; newest first. */
export function upsertAgent(list: AgentInstance[], agent: AgentInstance): AgentInstance[] {
  const i = list.findIndex((a) => a.id === agent.id);
  if (i < 0) return [agent, ...list];
  const next = [...list];
  next[i] = agent;
  return next;
}

/** Applies a project event to the agent list. Returns the same array when nothing changed. */
export function applyAgentEvent(list: AgentInstance[], ev: Event): AgentInstance[] {
  switch (ev.kind) {
    case "AgentSpawned":
    case "AgentUpdated":
      return upsertAgent(list, ev.payload.agent);
    case "AgentStateChanged": {
      const i = list.findIndex((a) => a.id === ev.payload.agentId);
      const cur = list[i];
      if (!cur) return list;
      const next = [...list];
      next[i] = { ...cur, state: ev.payload.to, activity: ev.payload.activity ?? cur.activity };
      return next;
    }
    default:
      return list;
  }
}

export interface TranscriptState {
  messages: AgentTranscriptMessage[];
  toolCalls: AgentToolCallView[];
  /** Messages still streaming (id → text so far). */
  streaming: Record<string, { role: AgentTranscriptMessage["role"]; text: string }>;
}

export const EMPTY_TRANSCRIPT: TranscriptState = { messages: [], toolCalls: [], streaming: {} };

/**
 * Applies a live event to a transcript: `AgentMessage` deltas accumulate
 * until persisted messages arrive from a reload; tool call events upsert.
 */
export function applyTranscriptEvent(t: TranscriptState, ev: Event): TranscriptState {
  switch (ev.kind) {
    case "AgentMessage": {
      const p = ev.payload;
      if (t.messages.some((m) => m.id === p.messageId)) return t;
      const prev = t.streaming[p.messageId];
      const text = (prev?.text ?? "") + p.delta;
      if (!text && p.done) return t;
      return { ...t, streaming: { ...t.streaming, [p.messageId]: { role: p.role, text } } };
    }
    case "ToolCallStarted": {
      const call = ev.payload.toolCall;
      const i = t.toolCalls.findIndex((c) => c.toolCall.id === call.id);
      if (i >= 0) return t;
      return { ...t, toolCalls: [...t.toolCalls, { toolCall: call, result: null, durationMs: null }] };
    }
    case "ToolCallFinished": {
      const p = ev.payload;
      const i = t.toolCalls.findIndex((c) => c.toolCall.id === p.toolCallId);
      const cur = t.toolCalls[i];
      if (!cur) return t;
      const next = [...t.toolCalls];
      next[i] = {
        ...cur,
        durationMs: p.durationMs,
        toolCall: { ...cur.toolCall, status: p.status, outputSummary: p.outputSummary },
      };
      return { ...t, toolCalls: next };
    }
    default:
      return t;
  }
}

/** Merges a reloaded transcript: persisted messages replace their streaming copies. */
export function mergeTranscript(
  t: TranscriptState,
  messages: AgentTranscriptMessage[],
  toolCalls: AgentToolCallView[],
): TranscriptState {
  const persisted = new Set(messages.map((m) => m.id));
  const streaming = Object.fromEntries(Object.entries(t.streaming).filter(([id]) => !persisted.has(id)));
  return { messages, toolCalls, streaming };
}

export type TimelineItem =
  | { kind: "message"; id: string; role: AgentTranscriptMessage["role"]; text: string; streaming: boolean; at: string | null }
  | { kind: "tool"; id: string; call: AgentToolCallView };

/** Messages with their tool calls placed after the message that requested them. */
export function timeline(t: TranscriptState): TimelineItem[] {
  const out: TimelineItem[] = [];
  const placed = new Set<string>();
  const byId = new Map(t.toolCalls.map((c) => [c.toolCall.id, c]));
  for (const m of t.messages) {
    if (m.text.trim()) out.push({ kind: "message", id: m.id, role: m.role, text: m.text, streaming: false, at: m.createdAt });
    for (const id of m.toolCallIds) {
      const c = byId.get(id);
      if (c) {
        out.push({ kind: "tool", id, call: c });
        placed.add(id);
      }
    }
  }
  for (const c of t.toolCalls) {
    if (!placed.has(c.toolCall.id)) out.push({ kind: "tool", id: c.toolCall.id, call: c });
  }
  for (const [id, s] of Object.entries(t.streaming)) {
    out.push({ kind: "message", id, role: s.role, text: s.text, streaming: true, at: null });
  }
  return out;
}

export const TOOL_TONE: Record<ToolCallStatus, Tone> = {
  Pending: "neutral",
  AwaitingApproval: "warning",
  Running: "info",
  Succeeded: "success",
  Failed: "danger",
  Denied: "danger",
  Cancelled: "neutral",
};

export function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 10_000) return `${Math.round(n / 1000)}k`;
  if (n >= 1000) return `${(n / 1000).toFixed(1)}k`;
  return String(n);
}

export function formatCost(usd: number): string {
  if (usd === 0) return "$0";
  if (usd < 0.01) return "<$0.01";
  return `$${usd.toFixed(2)}`;
}

/** Elapsed time of an agent run ("2m 05s"). */
export function elapsed(agent: Pick<AgentInstance, "startedAt" | "stoppedAt" | "createdAt">, now: number): string {
  const start = Date.parse(agent.startedAt ?? agent.createdAt);
  const end = agent.stoppedAt ? Date.parse(agent.stoppedAt) : now;
  if (!Number.isFinite(start) || !Number.isFinite(end)) return "—";
  const s = Math.max(0, Math.floor((end - start) / 1000));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  if (h > 0) return `${h}h ${String(m).padStart(2, "0")}m`;
  if (m > 0) return `${m}m ${String(sec).padStart(2, "0")}s`;
  return `${sec}s`;
}

export const ALL_TOOLS: readonly string[] = [
  "read_file",
  "write_file",
  "edit_file",
  "list_dir",
  "search_files",
  "run_command",
  "git_status",
  "git_diff",
  "git_commit",
  "create_snapshot",
  "ask_user",
  "record_memory",
  "record_decision",
  "submit_result",
];

/** The editable fields of a template. */
export function draftOf(t: AgentTemplate): TemplateDraft {
  return {
    name: t.name,
    role: t.role,
    description: t.description,
    systemPrompt: t.systemPrompt,
    preferredModel: t.preferredModel,
    fallbackModels: t.fallbackModels,
    tools: t.tools,
    permissions: t.permissions,
    memoryAccess: t.memoryAccess,
    budget: t.budget,
    maxTurns: t.maxTurns,
    worktreePolicy: t.worktreePolicy,
    qualityGates: t.qualityGates,
    voice: t.voice,
    avatar: t.avatar,
    visualState: t.visualState,
    version: t.version,
  };
}

/** A blank custom template. */
export function blankDraft(): TemplateDraft {
  return {
    name: "New agent",
    role: "Backend",
    description: "",
    systemPrompt: "You are a careful senior engineer working on {{project.name}}. Task: {{task.title}}.",
    preferredModel: null,
    fallbackModels: [],
    tools: ["read_file", "list_dir", "search_files", "write_file", "edit_file", "submit_result"],
    permissions: [],
    memoryAccess: "ProjectAndGlobal",
    budget: null,
    maxTurns: 30,
    worktreePolicy: "DedicatedWorktree",
    qualityGates: [],
    voice: null,
    avatar: null,
    visualState: "Idle",
    version: "1.0.0",
  };
}

/** Draft for "Duplicate": `(copy)` name, version reset. */
export function duplicateDraft(t: AgentTemplate): TemplateDraft {
  return { ...draftOf(t), name: `${t.name} (copy)`.slice(0, 80), version: "1.0.0" };
}

/** Client-side validation mirroring the core (the core re-validates). */
export function validateDraft(d: TemplateDraft): string | null {
  if (!d.name.trim() || d.name.trim().length > 80) return "Name must be 1–80 characters.";
  if (!d.systemPrompt.trim()) return "The system prompt must not be empty.";
  if (!Number.isInteger(d.maxTurns) || d.maxTurns < 1 || d.maxTurns > 500) return "Max turns must be 1–500.";
  if (!/^\d+\.\d+\.\d+$/.test(d.version.trim())) return "Version must look like 1.0.0.";
  if (d.budget?.maxCostUsd != null && !(d.budget.maxCostUsd >= 0)) return "Budget must be a positive amount.";
  return null;
}

export function isBuiltin(t: AgentTemplate): boolean {
  return t.source === "Builtin" || t.source === "Official";
}
