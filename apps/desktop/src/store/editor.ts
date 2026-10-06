/**
 * Editor store (zustand) shared by the Files and Code panels: open tabs,
 * the active file, pending "reveal at line" requests, agent change markers
 * and the quick-open / search toggles. Document contents and Monaco models
 * live in the Code panel (not serializable); this store holds view state only.
 */
import { create } from "zustand";

/** Where to put the cursor when a file opens. */
export interface RevealTarget {
  line: number;
  column?: number;
  /** Selection length in UTF-16 units (search hits). */
  length?: number;
}

export type TabStatus = "loading" | "ready" | "error" | "binary" | "tooLarge";

export interface EditorTab {
  path: string;
  status: TabStatus;
  /** Unsaved edits. */
  dirty: boolean;
  /** The file changed on disk while there were unsaved edits. */
  stale: boolean;
  /** The file was deleted on disk. */
  deleted: boolean;
  language: string;
  error?: string;
  /** 1-based lines changed by agents since the tab opened (gutter markers). */
  agentLines: number[];
}

export interface EditorState {
  projectId: string | null;
  tabs: EditorTab[];
  activePath: string | null;
  reveal: (RevealTarget & { path: string; nonce: number }) | null;
  /** Paths changed by agents this session → time (Files panel markers). */
  agentModified: Record<string, number>;
  quickOpen: boolean;
  searchOpen: boolean;
  /** Diff view of the active tab against the last saved content. */
  diff: boolean;
  /**
   * Unsaved tabs of projects that are not active, keyed by project id. A
   * project switch parks the dirty tabs here (their Monaco models are kept
   * by the Code panel's document registry) and restores them on switch back.
   */
  parked: Record<string, ParkedSession>;

  /** Switches project; clean tabs are closed, dirty ones are parked and restored on return. */
  setProject: (projectId: string | null) => void;
  /** Drops the tabs (open or parked) of a deleted project. */
  forgetProject: (projectId: string) => void;
  /** Opens (or focuses) a tab, optionally revealing a position. */
  openFile: (path: string, at?: RevealTarget) => void;
  closeTab: (path: string) => void;
  closeOthers: (path: string) => void;
  activate: (path: string) => void;
  updateTab: (path: string, patch: Partial<Omit<EditorTab, "path">>) => void;
  /** Rewrites open tab paths after a rename/move of `from` (file or folder). */
  movePath: (from: string, to: string) => void;
  /** Marks tabs below `path` as deleted on disk. */
  markDeleted: (path: string) => void;
  markAgentModified: (path: string) => void;
  addAgentLines: (path: string, lines: number[]) => void;
  clearAgentMarks: (path?: string) => void;
  consumeReveal: () => void;
  setQuickOpen: (open: boolean) => void;
  setSearchOpen: (open: boolean) => void;
  setDiff: (on: boolean) => void;
}

/** Dirty tabs of an inactive project. */
export interface ParkedSession {
  tabs: EditorTab[];
  activePath: string | null;
}

/** Projects other than `projectId` that hold unsaved tabs. */
export function projectsWithUnsavedTabs(state: Pick<EditorState, "projectId" | "tabs" | "parked">): string[] {
  const ids = Object.keys(state.parked).filter((id) => state.parked[id]?.tabs.some((t) => t.dirty));
  if (state.projectId !== null && state.tabs.some((t) => t.dirty) && !ids.includes(state.projectId)) ids.push(state.projectId);
  return ids;
}

/** Number of unsaved tabs a project holds (open or parked). */
export function unsavedTabCount(state: Pick<EditorState, "projectId" | "tabs" | "parked">, projectId: string): number {
  const tabs = state.projectId === projectId ? state.tabs : (state.parked[projectId]?.tabs ?? []);
  return tabs.filter((t) => t.dirty).length;
}

const within = (p: string, base: string) => p === base || p.startsWith(`${base}/`);

let nonce = 0;

function newTab(path: string): EditorTab {
  return { path, status: "loading", dirty: false, stale: false, deleted: false, language: "plaintext", agentLines: [] };
}

export const useEditorStore = create<EditorState>()((set, get) => ({
  projectId: null,
  tabs: [],
  activePath: null,
  reveal: null,
  agentModified: {},
  quickOpen: false,
  searchOpen: false,
  diff: false,
  parked: {},

  setProject: (projectId) => {
    const { projectId: current, tabs: currentTabs, activePath: currentActive, parked } = get();
    if (current === projectId) return;
    // First binding (e.g. a file opened from the Files panel before the Code panel mounted): keep tabs.
    if (current === null && projectId !== null && !(projectId in parked)) {
      set({ projectId });
      return;
    }
    // Never drop unsaved edits on a switch: park the dirty tabs of the project
    // being left (the document registry keeps their models) and bring back the
    // ones parked for the project being entered.
    const nextParked = { ...parked };
    if (current !== null) {
      const dirty = currentTabs.filter((t) => t.dirty);
      if (dirty.length > 0) {
        const activePath = dirty.some((t) => t.path === currentActive) ? currentActive : (dirty[0]?.path ?? null);
        nextParked[current] = { tabs: dirty, activePath };
      } else delete nextParked[current];
    }
    const restored = projectId !== null ? nextParked[projectId] : undefined;
    if (projectId !== null) delete nextParked[projectId];
    set({
      projectId,
      tabs: restored?.tabs ?? [],
      activePath: restored?.activePath ?? null,
      reveal: null,
      agentModified: {},
      quickOpen: false,
      diff: false,
      parked: nextParked,
    });
  },

  forgetProject: (projectId) =>
    set((s) => {
      // The deleted project's own tabs must not be parked by the next switch.
      if (s.projectId === projectId) {
        return { projectId: null, tabs: [], activePath: null, reveal: null, agentModified: {}, quickOpen: false, diff: false };
      }
      if (!(projectId in s.parked)) return s;
      const parked = { ...s.parked };
      delete parked[projectId];
      return { parked };
    }),

  openFile: (path, at) =>
    set((s) => {
      const tabs = s.tabs.some((t) => t.path === path) ? s.tabs : [...s.tabs, newTab(path)];
      return {
        tabs,
        activePath: path,
        reveal: at ? { ...at, path, nonce: ++nonce } : s.reveal,
        quickOpen: false,
      };
    }),

  closeTab: (path) =>
    set((s) => {
      const idx = s.tabs.findIndex((t) => t.path === path);
      if (idx < 0) return s;
      const tabs = s.tabs.filter((t) => t.path !== path);
      let activePath = s.activePath;
      if (activePath === path) activePath = tabs[Math.min(idx, tabs.length - 1)]?.path ?? null;
      return { tabs, activePath, diff: activePath === s.activePath ? s.diff : false };
    }),

  closeOthers: (path) => set((s) => ({ tabs: s.tabs.filter((t) => t.path === path || t.dirty), activePath: path })),

  activate: (path) => set((s) => (s.tabs.some((t) => t.path === path) ? { activePath: path, diff: false } : s)),

  updateTab: (path, patch) => set((s) => ({ tabs: s.tabs.map((t) => (t.path === path ? { ...t, ...patch } : t)) })),

  movePath: (from, to) =>
    set((s) => {
      const move = (p: string) => (within(p, from) ? `${to}${p.slice(from.length)}` : p);
      const agentModified: Record<string, number> = {};
      for (const [p, t] of Object.entries(s.agentModified)) agentModified[move(p)] = t;
      return {
        tabs: s.tabs.map((t) => (within(t.path, from) ? { ...t, path: move(t.path), deleted: false } : t)),
        activePath: s.activePath ? move(s.activePath) : null,
        agentModified,
      };
    }),

  markDeleted: (path) =>
    set((s) => ({ tabs: s.tabs.map((t) => (path === "" || within(t.path, path) ? { ...t, deleted: true } : t)) })),

  markAgentModified: (path) => set((s) => ({ agentModified: { ...s.agentModified, [path]: Date.now() } })),

  addAgentLines: (path, lines) =>
    set((s) => ({
      tabs: s.tabs.map((t) =>
        t.path === path ? { ...t, agentLines: [...new Set([...t.agentLines, ...lines])].sort((a, b) => a - b) } : t,
      ),
    })),

  clearAgentMarks: (path) =>
    set((s) => {
      if (path === undefined) return { agentModified: {}, tabs: s.tabs.map((t) => ({ ...t, agentLines: [] })) };
      const agentModified = { ...s.agentModified };
      delete agentModified[path];
      return { agentModified, tabs: s.tabs.map((t) => (t.path === path ? { ...t, agentLines: [] } : t)) };
    }),

  consumeReveal: () => set({ reveal: null }),
  setQuickOpen: (open) => set({ quickOpen: open }),
  setSearchOpen: (open) => set({ searchOpen: open }),
  setDiff: (on) => set({ diff: on }),
}));

/** The active tab, if any. */
export function useActiveTab(): EditorTab | null {
  return useEditorStore((s) => s.tabs.find((t) => t.path === s.activePath) ?? null);
}
