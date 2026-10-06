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

/** Per-project view state kept while another project is active. */
export interface ProjectSession {
  tabs: EditorTab[];
  activePath: string | null;
  agentModified: Record<string, number>;
}

export interface EditorState {
  projectId: string | null;
  /** Tabs of projects that are not active, restored when switching back (unsaved edits survive). */
  sessions: Record<string, ProjectSession>;
  tabs: EditorTab[];
  activePath: string | null;
  reveal: (RevealTarget & { path: string; nonce: number }) | null;
  /** Paths changed by agents this session → time (Files panel markers). */
  agentModified: Record<string, number>;
  quickOpen: boolean;
  searchOpen: boolean;
  /** Diff view of the active tab against the last saved content. */
  diff: boolean;

  /** Switches project; the previous project's tabs are parked and restored on return. */
  setProject: (projectId: string | null) => void;
  /** Patches a tab of `projectId`, whether that project is active or parked. */
  updateProjectTab: (projectId: string, path: string, patch: Partial<Omit<EditorTab, "path">>) => void;
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

const within = (p: string, base: string) => p === base || p.startsWith(`${base}/`);

let nonce = 0;

function newTab(path: string): EditorTab {
  return { path, status: "loading", dirty: false, stale: false, deleted: false, language: "plaintext", agentLines: [] };
}

export const useEditorStore = create<EditorState>()((set, get) => ({
  projectId: null,
  sessions: {},
  tabs: [],
  activePath: null,
  reveal: null,
  agentModified: {},
  quickOpen: false,
  searchOpen: false,
  diff: false,

  setProject: (projectId) => {
    const current = get().projectId;
    if (current === projectId) return;
    // First binding (e.g. a file opened from the Files panel before the Code panel mounted): keep tabs.
    if (current === null && projectId !== null) {
      set({ projectId });
      return;
    }
    const { tabs, activePath, agentModified } = get();
    const sessions = { ...get().sessions };
    if (current !== null) sessions[current] = { tabs, activePath, agentModified };
    const restored = projectId !== null ? sessions[projectId] : undefined;
    if (projectId !== null) delete sessions[projectId];
    set({
      projectId,
      sessions,
      tabs: restored?.tabs ?? [],
      activePath: restored?.activePath ?? null,
      agentModified: restored?.agentModified ?? {},
      reveal: null,
      quickOpen: false,
      diff: false,
    });
  },

  updateProjectTab: (projectId, path, patch) =>
    set((s) => {
      const patchTabs = (tabs: EditorTab[]) => tabs.map((t) => (t.path === path ? { ...t, ...patch } : t));
      if (s.projectId === projectId) return { tabs: patchTabs(s.tabs) };
      const parked = s.sessions[projectId];
      if (!parked) return s;
      return { sessions: { ...s.sessions, [projectId]: { ...parked, tabs: patchTabs(parked.tabs) } } };
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
