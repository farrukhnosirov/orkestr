/**
 * Codex chat model (pure): folds the streamed `CodexEvent`s of a session
 * (the ChatGPT subscription through the official `codex` CLI, ADR-0047)
 * into renderable chat items. Unit-tested without React.
 *
 * - `textDelta` builds an open answer per `itemId`; the complete `text` of
 *   the same item replaces it, so a lost delta never loses text.
 * - Tool calls use the Claude Code vocabulary (`Bash`, `Edit`, `WebSearch`,
 *   `mcp__…`), so the Claude tool cards render them; results pair by id.
 * - Approval requests become inline cards until `approvalResolved`.
 * - Reasoning is the CLI's summary only (never a hidden chain of thought).
 */
import type {
  ClaudeRisk,
  CodexApprovalDecision,
  CodexApprovalKind,
  CodexEvent,
  CodexLimits,
  CodexLimitWindow,
  CodexTokenUsage,
  JsonValue,
} from "@orkestr/protocol";

export type ApprovalStatus = "pending" | CodexApprovalDecision;

export interface CxUserItem {
  kind: "user";
  id: string;
  text: string;
}
export interface CxAssistantItem {
  kind: "assistant";
  id: string;
  text: string;
  streaming: boolean;
}
export interface CxReasoningItem {
  kind: "reasoning";
  id: string;
  text: string;
}
export interface CxPlanItem {
  kind: "plan";
  id: string;
  text: string;
}
export interface CxToolItem {
  kind: "tool";
  id: string;
  name: string;
  input: JsonValue;
  result: { content: string; isError: boolean } | null;
}
export interface CxApprovalItem {
  kind: "approval";
  id: string;
  approval: CodexApprovalKind;
  toolName: string;
  input: JsonValue;
  reason: string | null;
  risk: ClaudeRisk | null;
  canRemember: boolean;
  status: ApprovalStatus;
  byPolicy: boolean;
  message: string | null;
}
export interface CxTurnItem {
  kind: "turn";
  id: string;
  turnId: string;
  /** `completed`, `interrupted`, `failed`. */
  status: string;
  error: string | null;
  durationMs: number | null;
  files: string[];
}
export interface CxErrorItem {
  kind: "error";
  id: string;
  message: string;
  authRequired: boolean;
  usageLimit: boolean;
  willRetry: boolean;
}
export type CxNoticeKind = "info" | "modelChanged" | "exited";
export interface CxNoticeItem {
  kind: "notice";
  id: string;
  notice: CxNoticeKind;
  text: string;
}
export type CxItem = CxUserItem | CxAssistantItem | CxReasoningItem | CxPlanItem | CxToolItem | CxApprovalItem | CxTurnItem | CxErrorItem | CxNoticeItem;

export interface CxTokens {
  last: CodexTokenUsage;
  total: CodexTokenUsage;
  contextWindow: number | null;
}

export interface CodexChat {
  items: CxItem[];
  /** A turn is in progress. */
  running: boolean;
  /** The CLI's thread id. */
  threadId: string | null;
  turnId: string | null;
  model: string | null;
  effort: string | null;
  tokens: CxTokens | null;
  /** Plan limits pushed by the CLI during the session. */
  limits: CodexLimits | null;
  /** Files changed in the current turn. */
  turnFiles: string[];
  /** The CLI said the user must sign in. */
  authRequired: boolean;
  /** The plan's usage limit was reached (cleared by the next completed turn). */
  usageLimit: boolean;
  seq: number;
}

export function emptyCodexChat(): CodexChat {
  return {
    items: [],
    running: false,
    threadId: null,
    turnId: null,
    model: null,
    effort: null,
    tokens: null,
    limits: null,
    turnFiles: [],
    authRequired: false,
    usageLimit: false,
    seq: 0,
  };
}

/** Most items kept per session (oldest are dropped). */
export const MAX_CODEX_ITEMS = 2000;

function nextId(s: CodexChat, prefix: string): [string, number] {
  const seq = s.seq + 1;
  return [`${prefix}-${seq}`, seq];
}

function append(items: CxItem[], item: CxItem): CxItem[] {
  const out = [...items, item];
  return out.length > MAX_CODEX_ITEMS ? out.slice(out.length - MAX_CODEX_ITEMS) : out;
}

function update(items: CxItem[], id: string, fn: (it: CxItem) => CxItem): CxItem[] {
  const i = items.findIndex((it) => it.id === id);
  if (i < 0) return items;
  const out = items.slice();
  out[i] = fn(items[i] as CxItem);
  return out;
}

/** Id of the answer item of a CLI item id. */
export const answerId = (itemId: string) => `a:${itemId}`;

/** Closes every streamed answer. */
function closeStreams(items: CxItem[]): CxItem[] {
  return items.some((it) => it.kind === "assistant" && it.streaming)
    ? items.map((it) => (it.kind === "assistant" && it.streaming ? { ...it, streaming: false } : it))
    : items;
}

/** Settles still-pending approval cards (they can no longer be answered). */
function settleApprovals(items: CxItem[], decision: CodexApprovalDecision): CxItem[] {
  return items.some((it) => it.kind === "approval" && it.status === "pending")
    ? items.map((it) => (it.kind === "approval" && it.status === "pending" ? { ...it, status: decision, byPolicy: true } : it))
    : items;
}

/** Folds one event into the chat. */
export function applyCodexEvent(state: CodexChat, ev: CodexEvent): CodexChat {
  const s = state;
  switch (ev.type) {
    case "threadStarted":
      return { ...s, threadId: ev.threadId, model: ev.model ?? s.model, effort: ev.effort ?? s.effort };
    case "turnStarted":
      return { ...s, running: true, turnId: ev.turnId, turnFiles: [] };
    case "userMessage": {
      const [id, seq] = nextId(s, "u");
      return { ...s, seq, running: true, items: append(closeStreams(s.items), { kind: "user", id, text: ev.text }) };
    }
    case "textDelta": {
      const id = answerId(ev.itemId);
      if (s.items.some((it) => it.id === id)) {
        return { ...s, items: update(s.items, id, (it) => (it.kind === "assistant" ? { ...it, text: it.text + ev.text, streaming: true } : it)) };
      }
      return { ...s, running: true, items: append(s.items, { kind: "assistant", id, text: ev.text, streaming: true }) };
    }
    case "text": {
      const id = answerId(ev.itemId);
      if (s.items.some((it) => it.id === id)) {
        return { ...s, items: update(s.items, id, (it) => (it.kind === "assistant" ? { ...it, text: ev.text, streaming: false } : it)) };
      }
      return { ...s, items: append(s.items, { kind: "assistant", id, text: ev.text, streaming: false }) };
    }
    case "reasoning": {
      // Consecutive summaries extend one block.
      const last = s.items[s.items.length - 1];
      if (last && last.kind === "reasoning") {
        return { ...s, items: update(s.items, last.id, (it) => (it.kind === "reasoning" ? { ...it, text: `${it.text}\n\n${ev.text}` } : it)) };
      }
      const [id, seq] = nextId(s, "r");
      return { ...s, seq, items: append(s.items, { kind: "reasoning", id, text: ev.text }) };
    }
    case "plan": {
      // The plan is updated in place (one card per turn).
      const lastIndex = (kind: CxItem["kind"]) => {
        for (let i = s.items.length - 1; i >= 0; i--) if (s.items[i]?.kind === kind) return i;
        return -1;
      };
      const planIdx = lastIndex("plan");
      const existing = planIdx >= 0 ? s.items[planIdx] : undefined;
      if (existing && planIdx > lastIndex("user")) {
        return { ...s, items: update(s.items, existing.id, (it) => (it.kind === "plan" ? { ...it, text: ev.text } : it)) };
      }
      const [id, seq] = nextId(s, "p");
      return { ...s, seq, items: append(s.items, { kind: "plan", id, text: ev.text }) };
    }
    case "toolUse": {
      if (s.items.some((it) => it.id === ev.id && it.kind === "tool")) {
        return { ...s, items: update(s.items, ev.id, (it) => (it.kind === "tool" ? { ...it, name: ev.name, input: ev.input } : it)) };
      }
      const item: CxToolItem = { kind: "tool", id: ev.id, name: ev.name, input: ev.input, result: null };
      return { ...s, running: true, items: append(closeStreams(s.items), item) };
    }
    case "toolResult":
      return {
        ...s,
        items: update(s.items, ev.toolUseId, (it) => (it.kind === "tool" ? { ...it, result: { content: ev.content, isError: ev.isError } } : it)),
      };
    case "fileChanged":
      return s.turnFiles.includes(ev.path) ? s : { ...s, turnFiles: [...s.turnFiles, ev.path] };
    case "approvalRequest": {
      const item: CxApprovalItem = {
        kind: "approval",
        id: ev.requestId,
        approval: ev.kind,
        toolName: ev.toolName,
        input: ev.input,
        reason: ev.reason,
        risk: ev.risk,
        canRemember: ev.canRemember,
        status: "pending",
        byPolicy: false,
        message: null,
      };
      if (s.items.some((it) => it.id === ev.requestId)) return s;
      return { ...s, items: append(closeStreams(s.items), item) };
    }
    case "approvalResolved":
      return {
        ...s,
        items: update(s.items, ev.requestId, (it) =>
          it.kind === "approval" ? { ...it, status: ev.decision, byPolicy: ev.byPolicy, message: ev.message } : it,
        ),
      };
    case "tokenUsage":
      return { ...s, tokens: { last: ev.last, total: ev.total, contextWindow: ev.contextWindow } };
    case "turnCompleted": {
      const [id, seq] = nextId(s, "t");
      const items = settleApprovals(closeStreams(s.items), "abort");
      const item: CxTurnItem = {
        kind: "turn",
        id,
        turnId: ev.turnId,
        status: ev.status,
        error: ev.error,
        durationMs: ev.durationMs,
        files: s.turnFiles,
      };
      return {
        ...s,
        seq,
        running: false,
        turnId: null,
        turnFiles: [],
        usageLimit: ev.status === "completed" ? false : s.usageLimit,
        items: append(items, item),
      };
    }
    case "rateLimits":
      return { ...s, limits: ev.limits };
    case "modelChanged": {
      const [id, seq] = nextId(s, "n");
      const text = [ev.model, ev.effort].filter(Boolean).join(" · ");
      const items = text ? append(s.items, { kind: "notice", id, notice: "modelChanged", text }) : s.items;
      return { ...s, seq, model: ev.model ?? s.model, effort: ev.effort ?? s.effort, items };
    }
    case "info": {
      const [id, seq] = nextId(s, "n");
      return { ...s, seq, items: append(s.items, { kind: "notice", id, notice: "info", text: ev.text }) };
    }
    case "error": {
      const [id, seq] = nextId(s, "e");
      const item: CxErrorItem = {
        kind: "error",
        id,
        message: ev.message,
        authRequired: ev.authRequired,
        usageLimit: ev.usageLimit,
        willRetry: ev.willRetry,
      };
      return {
        ...s,
        seq,
        authRequired: s.authRequired || ev.authRequired,
        usageLimit: s.usageLimit || ev.usageLimit,
        items: append(closeStreams(s.items), item),
      };
    }
    case "exited": {
      const [id, seq] = nextId(s, "n");
      const items = settleApprovals(closeStreams(s.items), "deny");
      return {
        ...s,
        seq,
        running: false,
        items: append(items, { kind: "notice", id, notice: "exited", text: ev.code == null ? "" : String(ev.code) }),
      };
    }
  }
}

/** Folds a saved thread's transcript (nothing stays "running"). */
export function applyCodexAll(state: CodexChat, events: readonly CodexEvent[]): CodexChat {
  let s = state;
  for (const ev of events) s = applyCodexEvent(s, ev);
  return { ...s, running: false, items: settleApprovals(closeStreams(s.items), "deny") };
}

/** Marks the chat idle (the session reported idle / ended without a `turnCompleted`). */
export function settleCodexChat(s: CodexChat): CodexChat {
  if (!s.running && !s.items.some((it) => it.kind === "assistant" && it.streaming)) return s;
  return { ...s, running: false, items: closeStreams(s.items) };
}

/** The pending approval cards. */
export function pendingApprovals(s: CodexChat): CxApprovalItem[] {
  return s.items.filter((it): it is CxApprovalItem => it.kind === "approval" && it.status === "pending");
}

// ── helpers ───────────────────────────────────────────────────────────────

/** One file of a Codex patch (`Edit` input `changes`). */
export interface PatchFile {
  path: string;
  /** `add`, `delete`, `update`, … */
  kind: string;
  diff: string | null;
}

function obj(v: JsonValue | undefined): Record<string, JsonValue> | null {
  return v !== null && v !== undefined && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, JsonValue>) : null;
}

/** Files of a Codex `Edit` input (`{file_path, changes: [{path, kind, diff?}]}`); empty for Claude-shaped edits. */
export function patchFiles(input: JsonValue): PatchFile[] {
  const changes = obj(input)?.changes;
  if (!Array.isArray(changes)) return [];
  return changes.flatMap((c) => {
    const o = obj(c);
    const path = typeof o?.path === "string" ? o.path : null;
    if (!o || !path) return [];
    const kindValue = o.kind;
    const kind = typeof kindValue === "string" ? kindValue : (Object.keys(obj(kindValue) ?? {})[0] ?? "update");
    return [{ path, kind, diff: typeof o.diff === "string" ? o.diff : null }];
  });
}

export type PatchLineType = "add" | "del" | "hunk" | "same";

/** Classifies the lines of a unified diff (headers dropped). */
export function patchLines(diff: string, max = 400): { type: PatchLineType; text: string }[] {
  const out: { type: PatchLineType; text: string }[] = [];
  for (const line of diff.split("\n")) {
    if (line.startsWith("+++") || line.startsWith("---") || line.startsWith("diff ") || line.startsWith("index ")) continue;
    if (line.startsWith("@@")) out.push({ type: "hunk", text: line });
    else if (line.startsWith("+")) out.push({ type: "add", text: line.slice(1) });
    else if (line.startsWith("-")) out.push({ type: "del", text: line.slice(1) });
    else out.push({ type: "same", text: line.startsWith(" ") ? line.slice(1) : line });
    if (out.length >= max) break;
  }
  while (out.length > 0 && out[out.length - 1]?.type === "same" && out[out.length - 1]?.text === "") out.pop();
  return out;
}

/** Usage tone of a limit percentage. */
export type LimitTone = "ok" | "warn" | "danger";

export function limitTone(percent: number | null | undefined): LimitTone {
  if (percent == null) return "ok";
  if (percent >= 90) return "danger";
  if (percent > 80) return "warn";
  return "ok";
}

/** The fullest plan window (percent), when known. */
export function maxLimitPercent(l: CodexLimits | null | undefined): number | null {
  if (!l || !l.available) return null;
  const values = [l.primary?.percent, l.secondary?.percent].filter((v): v is number => typeof v === "number");
  return values.length > 0 ? Math.max(...values) : null;
}

/** How a plan window is labeled: 5-hour, weekly or a custom length. */
export function windowKind(w: CodexLimitWindow | null | undefined, fallback: "primary" | "secondary"): "hours" | "week" | "minutes" | "primary" | "secondary" {
  const m = w?.windowMinutes;
  if (m == null) return fallback;
  if (m === 10080) return "week";
  if (m % 60 === 0) return "hours";
  return "minutes";
}

/** Total tokens of a usage record (input already includes cached input). */
export function totalTokens(u: CodexTokenUsage): number {
  return u.inputTokens + u.outputTokens;
}

/** Share of the context window used (0‥1), when known. */
export function contextShare(t: CxTokens | null): number | null {
  if (!t || !t.contextWindow) return null;
  return Math.min(1, Math.max(0, t.last.inputTokens / t.contextWindow));
}
