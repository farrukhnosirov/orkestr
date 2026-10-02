/**
 * Pure helpers for the Git panel (no React, no RPC) — unit tested.
 */
import type {
  FileChangeStatus,
  RepoStatus,
  RollbackPlan,
  SnapshotChangeKind,
  StatusEntry,
  WorkspaceSnapshot,
} from "@orkestr/protocol";

import { formatBytes as formatBytesIntl, formatRelativeTime, t } from "../../i18n";

export type Tone = "neutral" | "accent" | "violet" | "success" | "warning" | "danger" | "info";

/** Which list of the Changes tab an entry is shown in. */
export type ChangeGroup = "conflicts" | "staged" | "unstaged" | "untracked";

export interface GroupedStatus {
  conflicts: StatusEntry[];
  staged: StatusEntry[];
  unstaged: StatusEntry[];
  untracked: StatusEntry[];
}

const NOT_CHANGED: ReadonlySet<FileChangeStatus> = new Set<FileChangeStatus>(["Unmodified", "Untracked", "Ignored"]);

/** Splits status entries into the Changes tab groups (an entry can be both staged and unstaged). */
export function groupStatus(entries: readonly StatusEntry[]): GroupedStatus {
  const g: GroupedStatus = { conflicts: [], staged: [], unstaged: [], untracked: [] };
  for (const e of entries) {
    if (e.conflict !== null) {
      g.conflicts.push(e);
      continue;
    }
    if (e.worktree === "Untracked") {
      g.untracked.push(e);
      continue;
    }
    if (e.worktree === "Ignored") continue;
    if (!NOT_CHANGED.has(e.index)) g.staged.push(e);
    if (!NOT_CHANGED.has(e.worktree)) g.unstaged.push(e);
  }
  const byPath = (a: StatusEntry, b: StatusEntry) => a.path.localeCompare(b.path);
  g.conflicts.sort(byPath);
  g.staged.sort(byPath);
  g.unstaged.sort(byPath);
  g.untracked.sort(byPath);
  return g;
}

/** One-letter code + tone for a change status. */
export function statusBadge(s: FileChangeStatus | SnapshotChangeKind): { letter: string; tone: Tone; label: string } {
  switch (s) {
    case "Modified":
      return { letter: "M", tone: "warning", label: t("git:fileStatus.Modified") };
    case "Added":
      return { letter: "A", tone: "success", label: t("git:fileStatus.Added") };
    case "Deleted":
      return { letter: "D", tone: "danger", label: t("git:fileStatus.Deleted") };
    case "Renamed":
      return { letter: "R", tone: "info", label: t("git:fileStatus.Renamed") };
    case "Copied":
      return { letter: "C", tone: "info", label: t("git:fileStatus.Copied") };
    case "TypeChanged":
      return { letter: "T", tone: "violet", label: t("git:fileStatus.TypeChanged") };
    case "Unmerged":
      return { letter: "!", tone: "danger", label: t("git:fileStatus.Unmerged") };
    case "Untracked":
      return { letter: "U", tone: "success", label: t("git:fileStatus.Untracked") };
    case "Ignored":
      return { letter: "I", tone: "neutral", label: t("git:fileStatus.Ignored") };
    default:
      return { letter: "·", tone: "neutral", label: t("git:fileStatus.Unchanged") };
  }
}

/** Status shown for an entry inside a group. */
export function entryStatus(e: StatusEntry, group: ChangeGroup): FileChangeStatus {
  if (group === "staged") return e.index;
  if (group === "conflicts") return "Unmerged";
  return e.worktree;
}

/** A node of the folder tree used by the change lists. */
export interface TreeNode<T> {
  /** Folder or file name (folders with a single child are compacted: `src/app`). */
  name: string;
  /** Full path of the folder or file. */
  path: string;
  children: TreeNode<T>[];
  item?: T;
}

/** Builds a sorted folder tree (folders first) from items with `/`-separated paths. */
export function buildTree<T>(items: readonly T[], pathOf: (item: T) => string): TreeNode<T>[] {
  const root: TreeNode<T> = { name: "", path: "", children: [] };
  for (const item of items) {
    const parts = pathOf(item).split("/").filter(Boolean);
    let node = root;
    parts.forEach((part, i) => {
      const path = parts.slice(0, i + 1).join("/");
      const isLeaf = i === parts.length - 1;
      let child = node.children.find((c) => c.name === part && (isLeaf ? c.item !== undefined : c.item === undefined));
      if (!child) {
        child = { name: part, path, children: [] };
        node.children.push(child);
      }
      if (isLeaf) child.item = item;
      node = child;
    });
  }
  const compact = (n: TreeNode<T>): TreeNode<T> => {
    let cur = n;
    while (cur.item === undefined && cur.children.length === 1 && cur.children[0]?.item === undefined) {
      const only = cur.children[0];
      if (!only) break;
      cur = { name: `${cur.name}/${only.name}`, path: only.path, children: only.children };
    }
    return { ...cur, children: sortNodes(cur.children.map(compact)) };
  };
  return sortNodes(root.children.map(compact));
}

function sortNodes<T>(nodes: TreeNode<T>[]): TreeNode<T>[] {
  return nodes.sort((a, b) => {
    const af = a.item === undefined ? 0 : 1;
    const bf = b.item === undefined ? 0 : 1;
    return af - bf || a.name.localeCompare(b.name);
  });
}

/** Leaf items under a node (for folder-level actions). */
export function leaves<T>(node: TreeNode<T>): T[] {
  const out: T[] = [];
  const walk = (n: TreeNode<T>) => {
    if (n.item !== undefined) out.push(n.item);
    n.children.forEach(walk);
  };
  walk(node);
  return out;
}

const LANGUAGES: Record<string, string> = {
  ts: "typescript",
  tsx: "typescript",
  mts: "typescript",
  cts: "typescript",
  js: "javascript",
  jsx: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  json: "json",
  md: "markdown",
  rs: "rust",
  py: "python",
  go: "go",
  java: "java",
  kt: "kotlin",
  cs: "csharp",
  cpp: "cpp",
  cc: "cpp",
  h: "cpp",
  hpp: "cpp",
  c: "c",
  css: "css",
  scss: "scss",
  less: "less",
  html: "html",
  htm: "html",
  xml: "xml",
  svg: "xml",
  yml: "yaml",
  yaml: "yaml",
  toml: "ini",
  ini: "ini",
  sql: "sql",
  sh: "shell",
  bash: "shell",
  ps1: "powershell",
  rb: "ruby",
  php: "php",
  swift: "swift",
  dockerfile: "dockerfile",
};

/** Monaco language id for a path (plaintext when unknown). */
export function languageForPath(path: string): string {
  const name = path.split("/").pop()?.toLowerCase() ?? "";
  if (name === "dockerfile") return "dockerfile";
  const ext = name.includes(".") ? name.split(".").pop() ?? "" : "";
  return LANGUAGES[ext] ?? "plaintext";
}

/** Size in the UI locale: `1.5 KB` / `1,5 КБ`. */
export function formatBytes(n: number, lng?: string): string {
  return formatBytesIntl(n, lng);
}

/** Relative time in the UI locale: `just now`, `5 minutes ago` / `5 минут назад`. */
export function relativeTime(iso: string, now: number = Date.now(), lng?: string): string {
  return formatRelativeTime(iso, now, lng);
}

/** Short commit id. */
export function shortId(id: string | null | undefined): string {
  return id ? id.slice(0, 8) : "—";
}

/** Counts per action of a rollback plan. */
export function summarizePlan(plan: RollbackPlan): { restore: number; recreate: number; delete: number; keep: number } {
  const s = { restore: 0, recreate: 0, delete: 0, keep: 0 };
  for (const i of plan.items) {
    if (i.action === "Restore") s.restore++;
    else if (i.action === "Recreate") s.recreate++;
    else if (i.action === "Delete") s.delete++;
    else s.keep++;
  }
  return s;
}

/** Human label of a snapshot reason. */
export function reasonLabel(s: WorkspaceSnapshot): string {
  switch (s.reason) {
    case "Manual":
      return t("git:reason.Manual");
    case "BeforeRiskyAction":
      return t("git:reason.BeforeRiskyAction");
    case "TaskCheckpoint":
      return s.taskPhase === "After"
        ? t("git:reason.taskAfter")
        : s.taskPhase === "Before"
          ? t("git:reason.taskBefore")
          : t("git:reason.TaskCheckpoint");
    case "PreRollback":
      return t("git:reason.PreRollback");
    case "Periodic":
      return t("git:reason.Periodic");
  }
}

/** Header summary of a status (`3 staged · 2 changed · 1 untracked`). */
export function statusSummary(st: RepoStatus): string {
  const g = groupStatus(st.entries);
  const parts: string[] = [];
  if (g.conflicts.length) parts.push(t("git:summary.conflicted", { count: g.conflicts.length }));
  if (g.staged.length) parts.push(t("git:summary.staged", { count: g.staged.length }));
  if (g.unstaged.length) parts.push(t("git:summary.changed", { count: g.unstaged.length }));
  if (g.untracked.length) parts.push(t("git:summary.untracked", { count: g.untracked.length }));
  return parts.length ? parts.join(" · ") : t("git:summary.clean");
}

/** Validates a branch name client-side (the core re-validates with git). */
export function branchNameError(name: string): string | null {
  const n = name.trim();
  if (!n) return t("git:branchName.empty");
  if (n.startsWith("-") || n.startsWith("/") || n.endsWith("/") || n.endsWith(".")) return t("git:branchName.edge");
  if (/[\s~^:?*[\\]|\.\.|@\{|\/\//.test(n) || n.endsWith(".lock")) return t("git:branchName.chars");
  return null;
}

/** Converts a CSS color (`#rgb`, `#rrggbb`, `rgb()`, `rgba()`) to `#rrggbb[aa]` for Monaco themes. */
export function cssColorToHex(value: string, fallback: string): string {
  const v = value.trim();
  if (/^#[0-9a-f]{6}([0-9a-f]{2})?$/i.test(v)) return v.toLowerCase();
  const short = /^#([0-9a-f])([0-9a-f])([0-9a-f])$/i.exec(v);
  if (short) return `#${short[1]}${short[1]}${short[2]}${short[2]}${short[3]}${short[3]}`.toLowerCase();
  const m = /^rgba?\(\s*([\d.]+)[\s,]+([\d.]+)[\s,]+([\d.]+)(?:[\s,/]+([\d.]+%?))?\s*\)$/i.exec(v);
  if (!m) return fallback;
  const hex = (n: number) => Math.max(0, Math.min(255, Math.round(n))).toString(16).padStart(2, "0");
  const [r, g, b] = [Number(m[1]), Number(m[2]), Number(m[3])];
  let out = `#${hex(r)}${hex(g)}${hex(b)}`;
  if (m[4] !== undefined) {
    const a = m[4].endsWith("%") ? Number(m[4].slice(0, -1)) / 100 : Number(m[4]);
    if (a < 1) out += hex(a * 255);
  }
  return out;
}
