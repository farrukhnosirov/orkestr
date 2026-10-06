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
  noteSeq: (seq: number) => void;
  setPaletteOpen: (open: boolean) => void;
  setCreateProjectOpen: (open: boolean) => void;
  /** Opens (id) or closes (null) the delete-project confirmation. */
  setDeleteProjectId: (projectId: string | null) => void;
  setThemeId: (id: string) => void;
}

const ACTIVE_PROJECT_KEY = "orkestr.activeProject";
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
  activeProjectId: readLocal(ACTIVE_PROJECT_KEY),
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
    const nextActive = stillThere ? active : (mostRecentlyOpened(sorted)?.id ?? sorted[0]?.id ?? null);
    writeLocal(ACTIVE_PROJECT_KEY, nextActive);
    set({ projects: sorted, projectsLoaded: true, activeProjectId: nextActive });
  },

  upsertProject: (project) =>
    set((s) => {
      if (project.archived) {
        const projects = s.projects.filter((p) => p.id !== project.id);
        const activeProjectId = s.activeProjectId === project.id ? (projects[0]?.id ?? null) : s.activeProjectId;
        return { projects, activeProjectId };
      }
      const exists = s.projects.some((p) => p.id === project.id);
      const projects = exists ? s.projects.map((p) => (p.id === project.id ? project : p)) : [...s.projects, project];
      // With no project open yet, the first one that appears becomes active.
      if (s.activeProjectId === null) {
        writeLocal(ACTIVE_PROJECT_KEY, project.id);
        return { projects: sortProjects(projects), activeProjectId: project.id };
      }
      return { projects: sortProjects(projects) };
    }),

  removeProject: (projectId) =>
    set((s) => {
      const projects = s.projects.filter((p) => p.id !== projectId);
      const activeProjectId = s.activeProjectId === projectId ? (projects[0]?.id ?? null) : s.activeProjectId;
      writeLocal(ACTIVE_PROJECT_KEY, activeProjectId);
      return { projects, activeProjectId, deleteProjectId: s.deleteProjectId === projectId ? null : s.deleteProjectId };
    }),

  setActiveProject: (projectId) => {
    writeLocal(ACTIVE_PROJECT_KEY, projectId);
    set({ activeProjectId: projectId });
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
