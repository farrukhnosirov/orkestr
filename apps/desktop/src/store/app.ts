/**
 * Global UI store (zustand). Holds view state and a cache of business data
 * that came from the core over RPC; it never talks to Tauri for data.
 */
import type { ConnectionState } from "@orkestr/client";
import type { Project } from "@orkestr/protocol";
import { create } from "zustand";

export type CoreMode = "embedded" | "external" | "browser";

export interface ConnectionInfo {
  state: ConnectionState;
  error: string | null;
  mode: CoreMode | null;
  url: string | null;
  serverVersion: string | null;
  startupMs: number | null;
}

export interface AppState {
  connection: ConnectionInfo;
  projects: Project[];
  projectsLoaded: boolean;
  activeProjectId: string | null;
  /**
   * The user closed the project (File → Close project): no project is open
   * and none is picked automatically until the user opens one; the main area
   * shows the Welcome page.
   */
  projectClosed: boolean;
  lastSeq: number;
  paletteOpen: boolean;
  createProjectOpen: boolean;
  /** Project whose delete confirmation is open, if any. */
  deleteProjectId: string | null;
  themeId: string;

  setConnection: (patch: Partial<ConnectionInfo>) => void;
  setProjects: (projects: Project[]) => void;
  upsertProject: (project: Project) => void;
  removeProject: (projectId: string) => void;
  setActiveProject: (projectId: string | null) => void;
  /** Closes the active project (shows the Welcome page). */
  closeProject: () => void;
  noteSeq: (seq: number) => void;
  setPaletteOpen: (open: boolean) => void;
  setCreateProjectOpen: (open: boolean) => void;
  /** Opens (id) or closes (null) the delete-project confirmation. */
  setDeleteProjectId: (projectId: string | null) => void;
  setThemeId: (id: string) => void;
}

export const ACTIVE_PROJECT_KEY = "orkestr.activeProject";
/** Set while the user has closed the project (survives restarts, like an empty VS Code window). */
export const PROJECT_CLOSED_KEY = "orkestr.projectClosed";
const THEME_KEY = "orkestr.theme";

function readLocal(key: string): string | null {
  try {
    return window.localStorage.getItem(key);
  } catch {
    return null;
  }
}

function writeLocal(key: string, value: string | null): void {
  try {
    if (value === null) window.localStorage.removeItem(key);
    else window.localStorage.setItem(key, value);
  } catch {
    /* storage unavailable */
  }
}

/**
 * Whether this window owns the persisted active project. Every window (main
 * shell, detached panels, HUD, robot windows) runs its own store and
 * bootstrapCore; only the main shell may write ACTIVE_PROJECT_KEY, otherwise a
 * secondary window holding a stale id overwrites the main window's choice on
 * every reconnect or ProjectDeleted. Secondary windows follow it instead
 * (followActiveProject).
 */
let ownsActiveProject = false;

/** Main shell only: this window persists the active project. */
export function ownActiveProject(): void {
  ownsActiveProject = true;
}

function persistActive(projectId: string | null): void {
  if (ownsActiveProject) writeLocal(ACTIVE_PROJECT_KEY, projectId);
}

function persistClosed(closed: boolean): void {
  if (ownsActiveProject) writeLocal(PROJECT_CLOSED_KEY, closed ? "1" : null);
}

/**
 * Secondary windows: track the active project the main window persists
 * (same-origin `storage` events). Returns the unsubscribe function.
 */
export function followActiveProject(): () => void {
  const onStorage = (e: StorageEvent) => {
    if (e.key !== ACTIVE_PROJECT_KEY || e.newValue === useAppStore.getState().activeProjectId) return;
    useAppStore.setState({ activeProjectId: e.newValue });
  };
  window.addEventListener("storage", onStorage);
  return () => window.removeEventListener("storage", onStorage);
}

/** Stable rail order: creation time, then name (switching projects never reorders the rail). */
export function sortProjects(projects: Project[]): Project[] {
  return [...projects].sort(
    (a, b) => a.createdAt.localeCompare(b.createdAt) || a.name.localeCompare(b.name, undefined, { sensitivity: "base" }),
  );
}

/** The most recently opened project, if any was ever opened. */
export function mostRecentlyOpened(projects: Project[]): Project | undefined {
  return projects
    .filter((p) => p.lastOpenedAt)
    .sort((a, b) => (b.lastOpenedAt ?? "").localeCompare(a.lastOpenedAt ?? ""))[0];
}

export const useAppStore = create<AppState>()((set, get) => ({
  connection: { state: "idle", error: null, mode: null, url: null, serverVersion: null, startupMs: null },
  projects: [],
  projectsLoaded: false,
  activeProjectId: readLocal(PROJECT_CLOSED_KEY) === "1" ? null : readLocal(ACTIVE_PROJECT_KEY),
  projectClosed: readLocal(PROJECT_CLOSED_KEY) === "1",
  lastSeq: 0,
  paletteOpen: false,
  createProjectOpen: false,
  deleteProjectId: null,
  themeId: readLocal(THEME_KEY) ?? "nebula",

  setConnection: (patch) => set((s) => ({ connection: { ...s.connection, ...patch } })),

  setProjects: (projects) => {
    const sorted = sortProjects(projects.filter((p) => !p.archived));
    const active = get().activeProjectId;
    const stillThere = active !== null && sorted.some((p) => p.id === active);
    const fallback = get().projectClosed ? null : (mostRecentlyOpened(sorted)?.id ?? sorted[0]?.id ?? null);
    const nextActive = stillThere ? active : fallback;
    persistActive(nextActive);
    set({ projects: sorted, projectsLoaded: true, activeProjectId: nextActive });
  },

  upsertProject: (project) =>
    set((s) => {
      if (project.archived) {
        const projects = s.projects.filter((p) => p.id !== project.id);
        const activeProjectId = s.activeProjectId === project.id ? (projects[0]?.id ?? null) : s.activeProjectId;
        if (activeProjectId !== s.activeProjectId) persistActive(activeProjectId);
        return { projects, activeProjectId };
      }
      const exists = s.projects.some((p) => p.id === project.id);
      const projects = exists ? s.projects.map((p) => (p.id === project.id ? project : p)) : [...s.projects, project];
      // With no project open yet, the first one that appears becomes active
      // (unless the user closed the project on purpose).
      if (s.activeProjectId === null && !s.projectClosed) {
        persistActive(project.id);
        return { projects: sortProjects(projects), activeProjectId: project.id };
      }
      return { projects: sortProjects(projects) };
    }),

  removeProject: (projectId) =>
    set((s) => {
      const projects = s.projects.filter((p) => p.id !== projectId);
      const activeProjectId = s.activeProjectId === projectId ? (projects[0]?.id ?? null) : s.activeProjectId;
      persistActive(activeProjectId);
      return { projects, activeProjectId, deleteProjectId: s.deleteProjectId === projectId ? null : s.deleteProjectId };
    }),

  setActiveProject: (projectId) => {
    persistActive(projectId);
    if (projectId !== null) persistClosed(false);
    set(projectId !== null ? { activeProjectId: projectId, projectClosed: false } : { activeProjectId: null });
  },

  closeProject: () => {
    persistActive(null);
    persistClosed(true);
    set({ activeProjectId: null, projectClosed: true });
  },

  noteSeq: (seq) => set((s) => (seq > s.lastSeq ? { lastSeq: seq } : s)),
  setPaletteOpen: (open) => set({ paletteOpen: open }),
  setCreateProjectOpen: (open) => set({ createProjectOpen: open, paletteOpen: open ? false : get().paletteOpen }),
  setDeleteProjectId: (projectId) => set({ deleteProjectId: projectId, paletteOpen: projectId ? false : get().paletteOpen }),
  setThemeId: (id) => {
    writeLocal(THEME_KEY, id);
    set({ themeId: id });
  },
}));

/** The active project, if any. */
export function useActiveProject(): Project | null {
  return useAppStore((s) => s.projects.find((p) => p.id === s.activeProjectId) ?? null);
}
