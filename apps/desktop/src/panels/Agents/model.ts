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

import { capabilityLabel, effectLabel } from "../../features/security/logic";
import { formatCurrency, formatDuration, formatList, formatNumber, t } from "../../i18n";
import { templateName } from "../../lib/coreText";

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

const KNOWN_ROLES: ReadonlySet<string> = new Set([
  "Manager",
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
]);

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

/** Localized role name (`agents:roles.*`); custom roles are shown as typed. */
export function roleLabel(role: AgentRole): string {
  if (typeof role === "object") return role.Custom;
  return KNOWN_ROLES.has(role) ? t(`agents:roles.${role as "Backend"}`) : role;
}

/** Two-letter monogram for the role avatar. */
export function roleInitials(role: AgentRole): string {
  const label = roleLabel(role).replace(/[^\p{L}/ -]/gu, "");
  const parts = label.split(/[ /-]+/).filter(Boolean);
  // An acronym first ("UI/UX designer") is the monogram itself.
  const first = parts[0] ?? "";
  if (first.length >= 2 && first.length <= 3 && first === first.toUpperCase() && /\p{Lu}/u.test(first)) return first.slice(0, 2);
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

/**
 * Replays live events (in arrival order) on top of a loaded `agents.list`
 * snapshot. The events were buffered while the list was loading; the
 * subscription was opened before the request, so replaying the whole ordered
 * sequence ends in the newest state even when the snapshot is older than
 * some of the events (agent events carry full state, so replaying one the
 * snapshot already contains is harmless).
 */
export function replayAgentEvents(list: AgentInstance[], events: readonly Event[]): AgentInstance[] {
  return events.reduce(applyAgentEvent, list);
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

/** Compact token count in the UI locale (`1.5K` / `1,5 тыс.`). */
export function formatTokens(n: number, lng?: string): string {
  return formatNumber(n, { notation: "compact", maximumFractionDigits: 1 }, lng);
}

/**
 * Count for the plural of a token label next to `formatTokens`: once the
 * value is compact («6,6 тыс.», «1,2 млн») the noun agrees with «тысяча» /
 * «миллион», i.e. the genitive plural («токенов»), whatever the last digits.
 */
export function tokenPluralCount(n: number): number {
  return n >= 1000 ? 1000 : n;
}

/** Money in the UI locale (`$1.50` / `1,50 $`). */
export function formatCost(usd: number, lng?: string): string {
  return formatCurrency(usd, "USD", lng);
}

/** Elapsed time of an agent run (`2 min 5 sec` / `2 мин 5 с`). */
export function elapsed(agent: Pick<AgentInstance, "startedAt" | "stoppedAt" | "createdAt">, now: number, lng?: string): string {
  const start = Date.parse(agent.startedAt ?? agent.createdAt);
  const end = agent.stoppedAt ? Date.parse(agent.stoppedAt) : now;
  if (!Number.isFinite(start) || !Number.isFinite(end)) return "—";
  // Whole seconds: a live counter should not flicker through milliseconds.
  return formatDuration(Math.max(1000, Math.floor((end - start) / 1000) * 1000), lng);
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
export function draftOf(tpl: AgentTemplate): TemplateDraft {
  return {
    name: tpl.name,
    role: tpl.role,
    description: tpl.description,
    systemPrompt: tpl.systemPrompt,
    preferredModel: tpl.preferredModel,
    fallbackModels: tpl.fallbackModels,
    tools: tpl.tools,
    permissions: tpl.permissions,
    memoryAccess: tpl.memoryAccess,
    budget: tpl.budget,
    maxTurns: tpl.maxTurns,
    worktreePolicy: tpl.worktreePolicy,
    qualityGates: tpl.qualityGates,
    voice: tpl.voice,
    avatar: tpl.avatar,
    visualState: tpl.visualState,
    version: tpl.version,
  };
}

/** A blank custom template. */
export function blankDraft(): TemplateDraft {
  return {
    name: t("agents:templates.defaultName"),
    role: "Backend",
    description: "",
    // The template variables are passed as values so i18next does not interpolate them.
    systemPrompt: t("agents:templates.defaultPrompt", { project: "{{project.name}}", task: "{{task.title}}" }),
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
export function duplicateDraft(tpl: AgentTemplate): TemplateDraft {
  return { ...draftOf(tpl), name: t("agents:templates.copyName", { name: templateName(tpl.name) }).slice(0, 80), version: "1.0.0" };
}

/** Client-side validation mirroring the core (the core re-validates). */
export function validateDraft(d: TemplateDraft): string | null {
  if (!d.name.trim() || d.name.trim().length > 80) return t("agents:validation.name");
  if (!d.systemPrompt.trim()) return t("agents:validation.prompt");
  if (!Number.isInteger(d.maxTurns) || d.maxTurns < 1 || d.maxTurns > 500) return t("agents:validation.maxTurns");
  if (!/^\d+\.\d+\.\d+$/.test(d.version.trim())) return t("agents:validation.version");
  if (d.budget?.maxCostUsd != null && !(d.budget.maxCostUsd >= 0)) return t("agents:validation.budget");
  return null;
}

export function isBuiltin(tpl: AgentTemplate): boolean {
  return tpl.source === "Builtin" || tpl.source === "Official";
}

/**
 * Requested permissions of a template draft in the UI language
 * («Сеть — Запретить, Развёртывание — Запретить»), never the raw enums.
 */
export function permissionsSummary(perms: TemplateDraft["permissions"], lng?: string): string {
  return formatList(
    perms.map((p) => `${capabilityLabel(p.capability)} — ${effectLabel(p.effect)}`),
    "conjunction",
    lng,
  );
}

/**
 * Voice profile for a voice typed into the template editor as
 * `provider:voice`. A new profile speaks in the UI language ("auto"), like
 * the Agent Voices settings; an existing profile keeps its language.
 */
export function voiceFromInput(value: string, current: TemplateDraft["voice"]): TemplateDraft["voice"] {
  const i = value.indexOf(":");
  const provider = i > 0 ? value.slice(0, i).trim() : "";
  const voice = i > 0 ? value.slice(i + 1) : "";
  if (!provider || !voice) return null;
  return { provider, voice, language: current?.language ?? "auto", style: current?.style ?? null };
}
