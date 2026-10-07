/**
 * Absolute host-path helpers for File → Open folder / Open file (pure,
 * unit-tested). Paths come from the native dialog or from the user, in the
 * host's format: `C:\Code\app`, `\\server\share\app`, `/home/me/app`.
 * Windows paths compare case-insensitively.
 */
import type { Project } from "@orkestr/protocol";

/** Whether `p` looks like a Windows path (drive letter or UNC). */
function isWindowsPath(p: string): boolean {
  return /^[a-zA-Z]:([\\/]|$)/.test(p) || p.startsWith("\\\\");
}

/**
 * Comparable form of a host path: `/` separators, no trailing separator
 * (except a root such as `c:/` or `/`), lower case on Windows.
 */
export function comparablePath(path: string): string {
  const raw = path.trim();
  const windows = isWindowsPath(raw);
  let p = raw.replace(/\\/g, "/");
  // Collapse duplicate separators (keep the UNC `//` prefix).
  const unc = p.startsWith("//");
  p = (unc ? "//" : "") + p.slice(unc ? 2 : 0).replace(/\/{2,}/g, "/");
  if (p.length > 1 && p.endsWith("/") && !/^[a-zA-Z]:\/$/.test(p)) p = p.replace(/\/+$/, "");
  if (/^[a-zA-Z]:$/.test(p)) p += "/";
  return windows ? p.toLowerCase() : p;
}

/** Whether two host paths name the same location. */
export function samePath(a: string, b: string): boolean {
  return comparablePath(a) === comparablePath(b);
}

/**
 * `path` relative to `root` (`/`-separated, the `files.*` wire format), `""`
 * for the root itself, or `null` when `path` is not inside `root`.
 */
export function relativeTo(root: string, path: string): string | null {
  const r = comparablePath(root);
  const p = comparablePath(path);
  if (p === r) return "";
  const prefix = r.endsWith("/") ? r : `${r}/`;
  if (!p.startsWith(prefix)) return null;
  // Keep the original spelling of the remainder (case matters on disk for display).
  const original = path.trim().replace(/\\/g, "/").replace(/\/{2,}/g, "/").replace(/\/+$/, "");
  const rest = original.slice(original.length - (p.length - prefix.length));
  return rest.replace(/^\/+/, "");
}

/** Last component of a host path (`C:\Code\my-app\` → `my-app`). */
export function hostBasename(path: string): string {
  const parts = path.trim().replace(/[\\/]+$/, "").split(/[\\/]/);
  return parts[parts.length - 1] ?? "";
}

/** Parent folder of a host path, in the host's own separators (`C:\a\b.txt` → `C:\a`). */
export function hostDirname(path: string): string {
  const p = path.trim().replace(/[\\/]+$/, "");
  const i = Math.max(p.lastIndexOf("/"), p.lastIndexOf("\\"));
  if (i < 0) return p;
  const parent = p.slice(0, i);
  // `C:\file` → `C:\`, `/file` → `/`.
  if (/^[a-zA-Z]:$/.test(parent)) return `${parent}${p[i]}`;
  return parent === "" ? p[i] ?? "/" : parent;
}

/** Whether `path` is absolute in the host's format. */
export function isAbsoluteHostPath(path: string): boolean {
  const p = path.trim();
  return /^[a-zA-Z]:[\\/]/.test(p) || p.startsWith("/") || p.startsWith("\\\\");
}

/** The (non-archived) project whose folder is exactly `folder`. */
export function projectAtFolder(projects: readonly Project[], folder: string): Project | undefined {
  return projects.find((p) => !p.archived && p.rootPath && samePath(p.rootPath, folder));
}

/**
 * The project containing `file` (the deepest root wins when project folders
 * are nested) and the file's path relative to that root.
 */
export function projectForFile(projects: readonly Project[], file: string): { project: Project; relPath: string } | null {
  let best: { project: Project; relPath: string; depth: number } | null = null;
  for (const project of projects) {
    if (project.archived || !project.rootPath) continue;
    const rel = relativeTo(project.rootPath, file);
    if (rel === null || rel === "") continue;
    const depth = comparablePath(project.rootPath).length;
    if (!best || depth > best.depth) best = { project, relPath: rel, depth };
  }
  return best ? { project: best.project, relPath: best.relPath } : null;
}
