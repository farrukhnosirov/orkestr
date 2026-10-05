/**
 * View state of the Tasks panel (zustand). Business data is NOT stored here —
 * it comes from the core via `useProjectTasks` (RPC + events).
 */
import { create } from "zustand";

import { EMPTY_FILTER, type SortDir, type SortKey, type TaskFilter } from "./model";

export type TasksView = "list" | "board" | "graph" | "product";

export interface Toast {
  id: number;
  tone: "danger" | "success" | "info";
  message: string;
}

export interface TasksUiState {
  view: TasksView;
  createOpen: boolean;
  selectedTaskId: string | null;
  filter: TaskFilter;
  sortKey: SortKey;
  sortDir: SortDir;
  toasts: Toast[];
  setView: (view: TasksView) => void;
  setCreateOpen: (open: boolean) => void;
  select: (taskId: string | null) => void;
  setFilter: (patch: Partial<TaskFilter>) => void;
  resetFilter: () => void;
  setSort: (key: SortKey) => void;
  toast: (tone: Toast["tone"], message: string) => void;
  dismissToast: (id: number) => void;
}

const VIEW_KEY = "orkestr.tasks.view";

function readView(): TasksView {
  try {
    const v = window.localStorage.getItem(VIEW_KEY);
    return v === "board" || v === "graph" || v === "list" || v === "product" ? v : "list";
  } catch {
    return "list";
  }
}

let toastSeq = 0;

export const useTasksUi = create<TasksUiState>()((set, get) => ({
  view: readView(),
  createOpen: false,
  selectedTaskId: null,
  filter: EMPTY_FILTER,
  sortKey: "priority",
  sortDir: "desc",
  toasts: [],
  setView: (view) => {
    try {
      window.localStorage.setItem(VIEW_KEY, view);
    } catch {
      /* storage unavailable */
    }
    set({ view });
  },
  setCreateOpen: (createOpen) => set({ createOpen }),
  select: (selectedTaskId) => set({ selectedTaskId }),
  setFilter: (patch) => set({ filter: { ...get().filter, ...patch } }),
  resetFilter: () => set({ filter: EMPTY_FILTER }),
  setSort: (key) => {
    const { sortKey, sortDir } = get();
    if (key === sortKey) set({ sortDir: sortDir === "asc" ? "desc" : "asc" });
    else set({ sortKey: key, sortDir: key === "title" ? "asc" : "desc" });
  },
  toast: (tone, message) => {
    const id = ++toastSeq;
    set({ toasts: [...get().toasts, { id, tone, message }].slice(-4) });
    window.setTimeout(() => get().dismissToast(id), tone === "danger" ? 6000 : 3500);
  },
  dismissToast: (id) => set({ toasts: get().toasts.filter((t) => t.id !== id) }),
}));
