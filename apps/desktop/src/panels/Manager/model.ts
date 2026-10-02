/**
 * Manager panel logic (pure, unit-tested): conversation state driven by
 * `manager.history` + live events, plan trees, plan edits, run progress and
 * request options for Expert mode (§53). No business data lives anywhere
 * else than the core — this only folds RPC results and events.
 */
import type {
  ChatAttachment,
  Event,
  ManagerChatMessage,
  ManagerConfig,
  ManagerPlan,
  ManagerRequestOptions,
  ManagerRun,
  ManagerRunStatus,
  PlanEdits,
  PlanStatus,
  PlanTask,
  TaskStatus,
} from "@orkestr/protocol";

import { formatCurrency, formatDuration as formatMs, t } from "../../i18n";

type Tone = "neutral" | "accent" | "violet" | "success" | "warning" | "danger" | "info";

// ── conversation ──────────────────────────────────────────

/** A reply being streamed (`ManagerMessage` deltas, before it is stored). */
export interface StreamingReply {
  id: string;
  text: string;
}

export interface ChatState {
  messages: ManagerChatMessage[];
  /** Replies in flight, by message id (order of first delta). */
  streaming: StreamingReply[];
  plans: Record<string, ManagerPlan>;
  runs: Record<string, ManagerRun>;
  busy: boolean;
  config: ManagerConfig | null;
}

export const EMPTY_CHAT: ChatState = { messages: [], streaming: [], plans: {}, runs: {}, busy: false, config: null };

function byCreated(a: ManagerChatMessage, b: ManagerChatMessage): number {
  return a.createdAt < b.createdAt ? -1 : a.createdAt > b.createdAt ? 1 : a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
}

/** Inserts or replaces a stored message (idempotent; keeps time order). */
export function upsertMessage(list: readonly ManagerChatMessage[], m: ManagerChatMessage): ManagerChatMessage[] {
  const i = list.findIndex((x) => x.id === m.id);
  if (i >= 0) {
    const next = [...list];
    next[i] = m;
    return next;
  }
  const last = list[list.length - 1];
  if (!last || byCreated(last, m) <= 0) return [...list, m];
  return [...list, m].sort(byCreated);
}

/** Folds a project event into the conversation state. Unrelated events return the same object. */
export function applyManagerEvent(s: ChatState, ev: Event): ChatState {
  switch (ev.kind) {
    case "ManagerMessage": {
      const p = ev.payload;
      if (p.agentId) return s;
      // A delta for a reply that is already stored is late — ignore it.
      if (s.messages.some((m) => m.id === p.messageId)) return s;
      const cur = s.streaming.find((r) => r.id === p.messageId);
      if (p.done) {
        // Keep the text until the stored message arrives (ManagerMessageAdded).
        return cur || !p.delta ? s : { ...s, streaming: [...s.streaming, { id: p.messageId, text: p.delta }] };
      }
      if (!p.delta) return s;
      const streaming = cur
        ? s.streaming.map((r) => (r.id === p.messageId ? { ...r, text: r.text + p.delta } : r))
        : [...s.streaming, { id: p.messageId, text: p.delta }];
      return { ...s, streaming, busy: true };
    }
    case "ManagerMessageAdded": {
      const { message, busy } = ev.payload;
      return {
        ...s,
        messages: upsertMessage(s.messages, message),
        streaming: s.streaming.filter((r) => r.id !== message.id),
        busy,
      };
    }
    case "ManagerPlanUpdated": {
      const plan = ev.payload.plan;
      return { ...s, plans: { ...s.plans, [plan.id]: plan } };
    }
    case "ManagerRunUpdated": {
      const run = ev.payload.run;
      const prev = s.runs[run.id];
      // Events can overtake each other on reconnect; keep the newest.
      if (prev && prev.updatedAt > run.updatedAt) return s;
      return { ...s, runs: { ...s.runs, [run.id]: run } };
    }
    default:
      return s;
  }
}

/** Index helper. */
export function indexById<T extends { id: string }>(items: readonly T[]): Record<string, T> {
  const out: Record<string, T> = {};
  for (const it of items) out[it.id] = it;
  return out;
}

/** One row of the conversation list. */
export type ChatItem =
  | { kind: "message"; id: string; message: ManagerChatMessage; showRun: boolean }
  | { kind: "streaming"; id: string; text: string };

/**
 * Rows to render: stored messages, then replies in flight. The first
 * message that mentions a run carries its live run card (`showRun`).
 */
export function chatItems(s: ChatState): ChatItem[] {
  const seenRuns = new Set<string>();
  const items: ChatItem[] = s.messages.map((m) => {
    let showRun = false;
    if (m.runId && !seenRuns.has(m.runId)) {
      seenRuns.add(m.runId);
      showRun = true;
    }
    return { kind: "message", id: m.id, message: m, showRun };
  });
  for (const r of s.streaming) items.push({ kind: "streaming", id: `stream-${r.id}`, text: r.text });
  return items;
}

/** The newest plan still waiting for a decision. */
export function latestProposedPlan(s: ChatState): ManagerPlan | null {
  let best: ManagerPlan | null = null;
  for (const p of Object.values(s.plans)) {
    if (p.status !== "Proposed") continue;
    if (!best || p.createdAt > best.createdAt) best = p;
  }
  return best;
}

/** The run to pin above the composer: the newest run that is not final. */
export function pinnedRun(s: ChatState): ManagerRun | null {
  let best: ManagerRun | null = null;
  for (const r of Object.values(s.runs)) {
    if (isRunFinal(r.status)) continue;
    if (!best || r.startedAt > best.startedAt) best = r;
  }
  return best;
}

// ── plans ────────────────────────────────────────────────

export const PLAN_STATUS_TONE: Readonly<Record<PlanStatus, Tone>> = {
  Proposed: "accent",
  Approved: "info",
  Rejected: "neutral",
  Superseded: "neutral",
  Running: "violet",
  Completed: "success",
  Failed: "danger",
  Cancelled: "neutral",
};

/** A plan task with its depth in the dependency graph (0 = no prerequisites). */
export interface PlanNode {
  task: PlanTask;
  level: number;
}

/** Group of plan tasks under one epic / feature heading. */
export interface PlanGroup {
  epic: string | null;
  feature: string | null;
  nodes: PlanNode[];
}

/** Dependency depth of every task (cycles / unknown keys count as depth 0). */
export function planLevels(tasks: readonly PlanTask[]): Map<string, number> {
  const byKey = new Map(tasks.map((t) => [t.key, t]));
  const memo = new Map<string, number>();
  const visiting = new Set<string>();
  const depth = (key: string): number => {
    const known = memo.get(key);
    if (known !== undefined) return known;
    if (visiting.has(key)) return 0;
    visiting.add(key);
    const t = byKey.get(key);
    let d = 0;
    for (const dep of t?.dependsOn ?? []) if (byKey.has(dep)) d = Math.max(d, depth(dep) + 1);
    visiting.delete(key);
    memo.set(key, d);
    return d;
  };
  for (const t of tasks) depth(t.key);
  return memo;
}

/**
 * Tasks grouped by epic → feature (in first-appearance order), each group
 * sorted by dependency depth so prerequisites come first.
 */
export function planTree(tasks: readonly PlanTask[]): PlanGroup[] {
  const levels = planLevels(tasks);
  const groups: PlanGroup[] = [];
  const find = (epic: string | null, feature: string | null) => groups.find((g) => g.epic === epic && g.feature === feature);
  tasks.forEach((task) => {
    const epic = task.epic ?? null;
    const feature = task.feature ?? null;
    let g = find(epic, feature);
    if (!g) {
      g = { epic, feature, nodes: [] };
      groups.push(g);
    }
    g.nodes.push({ task, level: levels.get(task.key) ?? 0 });
  });
  for (const g of groups) g.nodes.sort((a, b) => a.level - b.level);
  return groups;
}

/** Titles of a task's prerequisites ("after …"). */
export function dependencyTitles(task: PlanTask, tasks: readonly PlanTask[]): string[] {
  return task.dependsOn.map((k) => tasks.find((t) => t.key === k)?.title ?? k);
}

/** Inline edits of a proposed plan (titles, removed tasks). */
export interface PlanDraft {
  titles: Record<string, string>;
  removed: string[];
}

export const EMPTY_DRAFT: PlanDraft = { titles: {}, removed: [] };

/** Validation error of a draft, or null. */
export function validateDraft(plan: ManagerPlan, d: PlanDraft): string | null {
  const kept = plan.tasks.filter((task) => !d.removed.includes(task.key));
  if (kept.length === 0) return t("manager:validation.keepOne");
  for (const task of kept) {
    const title = (d.titles[task.key] ?? task.title).trim();
    if (!title) return t("manager:validation.titleEmpty");
    if (title.length > 200) return t("manager:validation.titleTooLong");
  }
  return null;
}

/** Converts a draft into `PlanEdits` (only real changes; `undefined` when nothing changed). */
export function draftToEdits(plan: ManagerPlan, d: PlanDraft): PlanEdits | undefined {
  const removeTasks = plan.tasks.filter((t) => d.removed.includes(t.key)).map((t) => t.key);
  const titles = plan.tasks
    .filter((t) => !d.removed.includes(t.key))
    .map((t) => ({ key: t.key, title: (d.titles[t.key] ?? t.title).trim(), original: t.title }))
    .filter((e) => e.title !== e.original)
    .map(({ key, title }) => ({ key, title }));
  if (removeTasks.length === 0 && titles.length === 0) return undefined;
  return { removeTasks, titles };
}

/** Tasks that depend on `key` (removing it drops that dependency in the core). */
export function dependentsOf(key: string, tasks: readonly PlanTask[]): PlanTask[] {
  return tasks.filter((t) => t.dependsOn.includes(key));
}

/** Estimated cost of a plan for display ("≈ $0.12" / "cost unknown"). */
export function planCost(plan: ManagerPlan): string {
  if (plan.estimatedCostUsd === null) return t("manager:plan.costUnknown");
  return t("manager:plan.costEstimate", { cost: formatUsd(plan.estimatedCostUsd) });
}

// ── runs ─────────────────────────────────────────────────

export const RUN_STATUS_TONE: Readonly<Record<ManagerRunStatus, Tone>> = {
  Running: "violet",
  Paused: "warning",
  Completed: "success",
  Failed: "danger",
  Cancelled: "neutral",
  Interrupted: "warning",
};

export function isRunFinal(s: ManagerRunStatus): boolean {
  return s === "Completed" || s === "Failed" || s === "Cancelled";
}

/** Progress of a run. */
export interface RunProgress {
  total: number;
  completed: number;
  running: number;
  failed: number;
  review: number;
  percent: number;
}

const RUNNING: readonly TaskStatus[] = ["Running", "Waiting", "Recovering"];

export function runProgress(run: ManagerRun): RunProgress {
  const total = run.tasks.length;
  const completed = run.tasks.filter((t) => t.status === "Completed").length;
  const running = run.tasks.filter((t) => RUNNING.includes(t.status)).length;
  const failed = run.tasks.filter((t) => t.status === "Failed" || t.status === "Interrupted").length;
  const review = run.tasks.filter((t) => t.awaitingReview).length;
  const percent = total === 0 ? 0 : Math.round((completed / total) * 100);
  return { total, completed, running, failed, review, percent };
}

/** Duration of whole seconds in the UI locale (`1 min 5 sec` / `1 мин 5 с`). */
export function formatDuration(secs: number, lng?: string): string {
  return formatMs(Math.max(0, Math.floor(secs)) * 1000, lng);
}

/** Money in the UI locale (`$1.23` / `1,23 $`). */
export function formatUsd(usd: number, lng?: string): string {
  return formatCurrency(usd, "USD", lng);
}

// ── composer / expert mode ───────────────────────────────

/** Expert-mode controls (§53), as typed by the user. */
export interface ExpertFields {
  budgetUsd: string;
  maxParallelAgents: string;
  timeLimitMins: string;
}

export const EMPTY_EXPERT: ExpertFields = { budgetUsd: "", maxParallelAgents: "", timeLimitMins: "" };

/**
 * Request options from Expert fields. Returns an error message for invalid
 * input, `undefined` options when nothing is set.
 */
export function buildOptions(
  f: ExpertFields,
  model: ManagerRequestOptions["model"] | null,
): { options?: ManagerRequestOptions; error?: string } {
  const options: ManagerRequestOptions = {};
  if (model && model.providerId && model.model.trim()) options.model = { providerId: model.providerId, model: model.model.trim() };
  if (f.budgetUsd.trim()) {
    const v = Number(f.budgetUsd);
    if (!Number.isFinite(v) || v < 0) return { error: t("manager:validation.budget") };
    options.budgetUsd = v;
  }
  if (f.maxParallelAgents.trim()) {
    const v = Number(f.maxParallelAgents);
    if (!Number.isInteger(v) || v < 1 || v > 64) return { error: t("manager:validation.parallel") };
    options.maxParallelAgents = v;
  }
  if (f.timeLimitMins.trim()) {
    const v = Number(f.timeLimitMins);
    if (!Number.isInteger(v) || v < 1 || v > 1440) return { error: t("manager:validation.timeLimit") };
    options.timeLimitMins = v;
  }
  return Object.keys(options).length > 0 ? { options } : {};
}

/** Detail of the `orkestr:ask-manager` window event (Code panel selection). */
export interface AskManagerRequest {
  text: string;
  projectId: string | null;
  path?: string;
  startLine?: number;
  endLine?: number;
  language?: string;
}

/** Attachment for a selection; plain text requests have none. */
export function attachmentOf(r: AskManagerRequest): ChatAttachment | null {
  if (!r.path || !r.text.trim()) return null;
  const a: ChatAttachment = { path: r.path, text: r.text.slice(0, 20_000) };
  if (r.startLine !== undefined) a.startLine = r.startLine;
  if (r.endLine !== undefined) a.endLine = r.endLine;
  if (r.language) a.language = r.language;
  return a;
}

/** Short label of an attachment ("src/a.rs:3-5"). */
export function attachmentLabel(a: ChatAttachment): string {
  if (a.startLine !== undefined && a.endLine !== undefined && a.endLine !== a.startLine) return `${a.path}:${a.startLine}-${a.endLine}`;
  if (a.startLine !== undefined) return `${a.path}:${a.startLine}`;
  return a.path;
}

/** Quick suggestions, depending on what is going on (sent to the Manager as typed, in the UI language). */
export function suggestions(s: ChatState): string[] {
  const out: string[] = [];
  const run = pinnedRun(s);
  if (latestProposedPlan(s)) out.push(t("manager:suggestions.planRisks"));
  if (run) out.push(t("manager:suggestions.runProgress"));
  out.push(t("manager:suggestions.projectStatus"));
  if (!run) out.push(t("manager:suggestions.helloModule"));
  out.push(t("manager:suggestions.approvals"));
  out.push(t("manager:suggestions.costs"));
  return out.slice(0, 4);
}

/** Composer key handling: what a key press does. */
export function composerKeyAction(e: { key: string; shiftKey: boolean; ctrlKey: boolean; metaKey: boolean; isComposing?: boolean }): "send" | "newline" | null {
  if (e.key !== "Enter" || e.isComposing) return null;
  if (e.ctrlKey || e.metaKey) return "send";
  if (e.shiftKey) return "newline";
  return "send";
}

/** Model label for the indicator ("mock-smart · Mock (offline demo)"). */
export function modelLabel(config: ManagerConfig | null, providerName: (id: string) => string | null): string {
  const m = config?.model;
  if (!m) return t("manager:model.none");
  const p = providerName(m.providerId);
  return p ? `${m.model} · ${p}` : m.model;
}
