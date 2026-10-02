/**
 * Pure helpers for the Tasks panel: status metadata, the client-side mirror of
 * the transition table (for showing only valid actions — the core remains
 * authoritative and rejects anything else), sorting and filtering.
 */
import type { Task, TaskPriority, TaskStatus } from "@orkestr/protocol";

import { formatCurrency } from "../../i18n";

export type Tone = "neutral" | "accent" | "violet" | "success" | "warning" | "danger" | "info";

export const ALL_STATUSES: readonly TaskStatus[] = [
  "Planned",
  "Ready",
  "Running",
  "Waiting",
  "Blocked",
  "Review",
  "Testing",
  "Completed",
  "Failed",
  "Cancelled",
  "Interrupted",
  "Recovering",
];

export const PRIORITIES: readonly TaskPriority[] = ["Critical", "High", "Normal", "Low", "Background"];

/** Contract §5 table (mirrors `orkestr_task_engine::state_machine`). */
export const TRANSITIONS: Readonly<Record<TaskStatus, readonly TaskStatus[]>> = {
  Planned: ["Ready", "Blocked", "Cancelled"],
  Ready: ["Running", "Blocked", "Cancelled", "Planned"],
  Running: ["Waiting", "Blocked", "Review", "Testing", "Completed", "Failed", "Cancelled", "Interrupted"],
  Waiting: ["Running", "Blocked", "Cancelled", "Failed"],
  Blocked: ["Ready", "Cancelled", "Failed"],
  Review: ["Testing", "Running", "Completed", "Failed", "Cancelled"],
  Testing: ["Review", "Running", "Completed", "Failed", "Cancelled"],
  Failed: ["Ready", "Recovering", "Cancelled"],
  Interrupted: ["Recovering", "Cancelled"],
  Recovering: ["Ready", "Running", "Failed", "Cancelled"],
  Completed: [],
  Cancelled: [],
};

export function canTransition(from: TaskStatus, to: TaskStatus): boolean {
  return TRANSITIONS[from].includes(to);
}

export const STATUS_TONE: Readonly<Record<TaskStatus, Tone>> = {
  Planned: "neutral",
  Ready: "accent",
  Running: "info",
  Waiting: "warning",
  Blocked: "warning",
  Review: "violet",
  Testing: "violet",
  Completed: "success",
  Failed: "danger",
  Cancelled: "neutral",
  Interrupted: "danger",
  Recovering: "info",
};

export const PRIORITY_TONE: Readonly<Record<TaskPriority, Tone>> = {
  Critical: "danger",
  High: "warning",
  Normal: "accent",
  Low: "neutral",
  Background: "neutral",
};

/** CSS color token per status (used by graph nodes and board accents). */
export function statusColor(status: TaskStatus): string {
  const tone = STATUS_TONE[status];
  switch (tone) {
    case "neutral":
      return "var(--ok-color-text-subtle)";
    case "accent":
      return "var(--ok-color-accent)";
    default:
      return `var(--ok-color-${tone})`;
  }
}

export type BoardColumnId = "backlog" | "ready" | "active" | "verify" | "done" | "attention";

/** Kanban columns: groups of statuses, with the status a drop transitions to. Titles: `tasks:board.columns.<id>`. */
export interface BoardColumn {
  id: BoardColumnId;
  statuses: readonly TaskStatus[];
  /** Target statuses tried in order when a card is dropped here. */
  dropTargets: readonly TaskStatus[];
}

export const BOARD_COLUMNS: readonly BoardColumn[] = [
  { id: "backlog", statuses: ["Planned", "Blocked"], dropTargets: ["Planned", "Blocked"] },
  { id: "ready", statuses: ["Ready"], dropTargets: ["Ready"] },
  { id: "active", statuses: ["Running", "Waiting", "Recovering"], dropTargets: ["Running", "Waiting"] },
  { id: "verify", statuses: ["Review", "Testing"], dropTargets: ["Review", "Testing"] },
  { id: "done", statuses: ["Completed"], dropTargets: ["Completed"] },
  { id: "attention", statuses: ["Failed", "Interrupted", "Cancelled"], dropTargets: ["Failed", "Cancelled"] },
];

export function columnOf(status: TaskStatus): BoardColumn | undefined {
  return BOARD_COLUMNS.find((c) => c.statuses.includes(status));
}

/** The status a card in `from` would move to when dropped on `column`, or null if not allowed. */
export function dropTarget(from: TaskStatus, column: BoardColumn): TaskStatus | null {
  if (column.statuses.includes(from)) return null;
  return column.dropTargets.find((to) => canTransition(from, to)) ?? null;
}

const PRIORITY_RANK: Readonly<Record<TaskPriority, number>> = { Critical: 4, High: 3, Normal: 2, Low: 1, Background: 0 };
const STATUS_RANK: Readonly<Record<TaskStatus, number>> = Object.fromEntries(
  ALL_STATUSES.map((s, i) => [s, i]),
) as Record<TaskStatus, number>;

export type SortKey = "title" | "status" | "priority" | "updatedAt" | "createdAt";
export type SortDir = "asc" | "desc";

export function sortTasks(tasks: readonly Task[], key: SortKey, dir: SortDir): Task[] {
  const sign = dir === "asc" ? 1 : -1;
  const cmp = (a: Task, b: Task): number => {
    switch (key) {
      case "title":
        return a.title.localeCompare(b.title, undefined, { sensitivity: "base" });
      case "status":
        return STATUS_RANK[a.status] - STATUS_RANK[b.status];
      case "priority":
        return PRIORITY_RANK[a.priority] - PRIORITY_RANK[b.priority];
      case "updatedAt":
        return a.updatedAt.localeCompare(b.updatedAt);
      case "createdAt":
        return a.createdAt.localeCompare(b.createdAt);
    }
  };
  return [...tasks].sort((a, b) => sign * cmp(a, b) || a.createdAt.localeCompare(b.createdAt) || a.id.localeCompare(b.id));
}

export interface TaskFilter {
  query: string;
  statuses: readonly TaskStatus[];
  priorities: readonly TaskPriority[];
}

export const EMPTY_FILTER: TaskFilter = { query: "", statuses: [], priorities: [] };

export function isFilterActive(f: TaskFilter): boolean {
  return f.query.trim() !== "" || f.statuses.length > 0 || f.priorities.length > 0;
}

export function filterTasks(tasks: readonly Task[], f: TaskFilter): Task[] {
  const q = f.query.trim().toLowerCase();
  return tasks.filter((t) => {
    if (f.statuses.length > 0 && !f.statuses.includes(t.status)) return false;
    if (f.priorities.length > 0 && !f.priorities.includes(t.priority)) return false;
    if (!q) return true;
    return (
      t.title.toLowerCase().includes(q) ||
      t.description.toLowerCase().includes(q) ||
      t.tags.some((tag) => tag.toLowerCase().includes(q))
    );
  });
}

/** Inserts or replaces a task in a list (keeps creation order). */
export function upsertTask(tasks: readonly Task[], task: Task): Task[] {
  const i = tasks.findIndex((t) => t.id === task.id);
  if (i === -1) return [...tasks, task];
  const existing = tasks[i];
  // Ignore stale updates (events can race with RPC responses).
  if (existing && existing.version > task.version) return [...tasks];
  const next = [...tasks];
  next[i] = task;
  return next;
}

export function shortId(id: string): string {
  return id.slice(-6);
}

/** Money in the UI locale (`$1.23` / `1,23 $`). */
export function formatCost(usd: number, lng?: string): string {
  return formatCurrency(usd, "USD", lng);
}
