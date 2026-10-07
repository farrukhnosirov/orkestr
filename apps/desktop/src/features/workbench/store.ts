/**
 * UI state of the File menu / Welcome page flows: which dialog is open and
 * which projects the user removed from the "Recent" list. The recent list
 * itself comes from the core (`lastOpenedAt`, set by `projects.open`); the
 * removals are a per-device view preference kept in localStorage.
 */
import type { Project } from "@orkestr/protocol";
import { create } from "zustand";

export type EntryKind = "file" | "folder";

/** Typed-path fallback of Open folder / Open file when no native picker exists (browser). */
export type PathPromptKind = "openFolder" | "openFile";

/** A file chosen with File → Open file… that lies outside every project. */
export interface OutsideFile {
  path: string;
  folder: string;
}

export interface WorkbenchState {
  /** File menu dropdown open (Alt+F / click). */
  menuOpen: boolean;
  cloneOpen: boolean;
  /** New file / New folder name prompt for the active project. */
  entryPrompt: EntryKind | null;
  outsideFile: OutsideFile | null;
  pathPrompt: PathPromptKind | null;
  /** `projectId → lastOpenedAt` at the time it was removed from Recent. */
  hiddenRecent: Record<string, string>;

  setMenuOpen: (open: boolean) => void;
  setCloneOpen: (open: boolean) => void;
  setEntryPrompt: (kind: EntryKind | null) => void;
  setOutsideFile: (file: OutsideFile | null) => void;
  setPathPrompt: (kind: PathPromptKind | null) => void;
  hideRecent: (project: Project) => void;
}

const HIDDEN_KEY = "orkestr.recentHidden";

function readHidden(): Record<string, string> {
  try {
    const raw = window.localStorage.getItem(HIDDEN_KEY);
    const v: unknown = raw ? JSON.parse(raw) : {};
    if (!v || typeof v !== "object" || Array.isArray(v)) return {};
    return Object.fromEntries(Object.entries(v as Record<string, unknown>).filter((e): e is [string, string] => typeof e[1] === "string"));
  } catch {
    return {};
  }
}

function writeHidden(v: Record<string, string>): void {
  try {
    window.localStorage.setItem(HIDDEN_KEY, JSON.stringify(v));
  } catch {
    /* storage unavailable: the removal lasts for this session */
  }
}

export const useWorkbenchStore = create<WorkbenchState>()((set, get) => ({
  menuOpen: false,
  cloneOpen: false,
  entryPrompt: null,
  outsideFile: null,
  pathPrompt: null,
  hiddenRecent: readHidden(),

  setMenuOpen: (open) => set({ menuOpen: open }),
  setCloneOpen: (open) => set({ cloneOpen: open, menuOpen: false }),
  setEntryPrompt: (kind) => set({ entryPrompt: kind, menuOpen: false }),
  setOutsideFile: (file) => set({ outsideFile: file }),
  setPathPrompt: (kind) => set({ pathPrompt: kind, menuOpen: false }),
  hideRecent: (project) => {
    const next = { ...get().hiddenRecent, [project.id]: project.lastOpenedAt ?? "" };
    writeHidden(next);
    set({ hiddenRecent: next });
  },
}));

/** Max entries of the Recent lists. */
export const MAX_RECENT = 10;

/**
 * Recently opened projects, most recent first. A project removed from the
 * list comes back once it is opened again (newer `lastOpenedAt`).
 */
export function recentProjects(
  projects: readonly Project[],
  hidden: Readonly<Record<string, string>>,
  excludeId: string | null = null,
  limit = MAX_RECENT,
): Project[] {
  return projects
    .filter((p) => !p.archived && p.lastOpenedAt && p.id !== excludeId)
    .filter((p) => {
      const removedAt = hidden[p.id];
      return removedAt === undefined || (p.lastOpenedAt ?? "") > removedAt;
    })
    .sort((a, b) => (b.lastOpenedAt ?? "").localeCompare(a.lastOpenedAt ?? ""))
    .slice(0, limit);
}
