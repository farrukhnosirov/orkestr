/**
 * Pure state helpers of the lazy file tree: flattening the expanded tree
 * into rows for the virtualized list, filtering, and applying change events.
 */
import type { FileChangedPayload, FileEntry } from "@orkestr/protocol";

import { dirname, isWithin, movePath } from "./paths";

/** Load state of one directory's children. */
export type DirState =
  | { status: "loading"; entries?: FileEntry[] }
  | { status: "loaded"; entries: FileEntry[]; truncated: boolean }
  | { status: "error"; message: string; entries?: FileEntry[] };

/** One visible row. */
export type TreeRow =
  | { type: "entry"; entry: FileEntry; depth: number; expanded: boolean; loading: boolean }
  | { type: "status"; path: string; depth: number; status: "loading" | "error" | "empty" | "truncated"; message?: string };

/**
 * Flattens the tree below `root` (exclusive) into rows.
 *
 * With a `filter`, only entries whose name matches (case-insensitive
 * substring) are shown, plus their loaded ancestors; folders on the way are
 * shown expanded.
 */
export function flattenTree(
  dirs: ReadonlyMap<string, DirState>,
  expanded: ReadonlySet<string>,
  filter = "",
  root = "",
): TreeRow[] {
  const rows: TreeRow[] = [];
  const q = filter.trim().toLowerCase();
  const matches = q ? computeMatches(dirs, q, root) : null;

  const walk = (dir: string, depth: number) => {
    const state = dirs.get(dir);
    if (!state) return;
    const entries = state.entries ?? [];
    if (state.status === "loading" && entries.length === 0) {
      rows.push({ type: "status", path: dir, depth, status: "loading" });
      return;
    }
    if (state.status === "error" && entries.length === 0) {
      rows.push({ type: "status", path: dir, depth, status: "error", message: state.message });
      return;
    }
    let shown = 0;
    for (const entry of entries) {
      if (matches && !matches.has(entry.path)) continue;
      const isDir = entry.kind === "dir";
      const open = isDir && (expanded.has(entry.path) || (matches !== null && hasMatchingDescendant(matches, entry.path)));
      const childState = dirs.get(entry.path);
      rows.push({ type: "entry", entry, depth, expanded: open, loading: open && childState?.status === "loading" });
      shown++;
      if (open) walk(entry.path, depth + 1);
    }
    if (shown === 0 && !matches && state.status === "loaded" && dir !== root) {
      rows.push({ type: "status", path: dir, depth, status: "empty" });
    }
    if (state.status === "loaded" && state.truncated) {
      rows.push({ type: "status", path: dir, depth, status: "truncated" });
    }
  };
  walk(root, 0);
  return rows;
}

function hasMatchingDescendant(matches: ReadonlySet<string>, dir: string): boolean {
  for (const m of matches) if (m !== dir && isWithin(m, dir)) return true;
  return false;
}

/** Paths that match the filter, plus all their ancestors (only loaded entries are considered). */
function computeMatches(dirs: ReadonlyMap<string, DirState>, q: string, root: string): Set<string> {
  const out = new Set<string>();
  for (const [, state] of dirs) {
    for (const e of state.entries ?? []) {
      if (!e.name.toLowerCase().includes(q)) continue;
      let p = e.path;
      while (p !== root && p !== "") {
        out.add(p);
        p = dirname(p);
      }
    }
  }
  return out;
}

/**
 * Directories whose listing must be refreshed after a change (the parent of
 * the changed path, and of the old path for renames), limited to loaded ones.
 */
export function dirsToRefresh(change: FileChangedPayload, loaded: ReadonlySet<string>): string[] {
  const out = new Set<string>();
  if (change.path === "") {
    for (const d of loaded) out.add(d);
    return [...out];
  }
  for (const p of [change.path, change.oldPath]) {
    if (p == null) continue;
    const parent = dirname(p);
    if (loaded.has(parent)) out.add(parent);
  }
  return [...out];
}

/** Applies a rename/delete to the expanded set (keeps folders open after moves). */
export function updateExpanded(expanded: ReadonlySet<string>, change: FileChangedPayload): Set<string> {
  const next = new Set<string>();
  for (const p of expanded) {
    if (change.change === "Deleted" && isWithin(p, change.path) && change.path !== "") continue;
    if (change.change === "Renamed" && change.oldPath != null) next.add(movePath(p, change.oldPath, change.path));
    else next.add(p);
  }
  return next;
}

/** Drops cached listings below a deleted or moved directory. */
export function pruneDirs(dirs: ReadonlyMap<string, DirState>, change: FileChangedPayload): Map<string, DirState> {
  const next = new Map(dirs);
  const gone = change.change === "Deleted" ? change.path : change.change === "Renamed" ? change.oldPath : null;
  if (gone != null && gone !== "") {
    for (const key of dirs.keys()) if (isWithin(key, gone)) next.delete(key);
  }
  return next;
}
