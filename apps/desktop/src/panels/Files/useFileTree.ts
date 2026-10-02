/**
 * Lazy file tree state: directory listings loaded on demand via
 * `files.list`, expanded folders (persisted per project), and live updates
 * from `FileChanged` events (only affected, already-loaded folders reload).
 */
import type { FileChangedPayload } from "@orkestr/protocol";
import { useCallback, useEffect, useRef, useState } from "react";

import { getClient } from "../../core/client";
import { type DirState, dirsToRefresh, pruneDirs, updateExpanded } from "../../features/files/tree";
import { errorInfo } from "../../features/files/rpc";

const EXPANDED_KEY = (projectId: string) => `orkestr.files.expanded.${projectId}`;

function loadExpanded(projectId: string): Set<string> {
  try {
    const raw = window.localStorage.getItem(EXPANDED_KEY(projectId));
    const parsed: unknown = raw ? JSON.parse(raw) : [];
    return new Set(Array.isArray(parsed) ? parsed.filter((p): p is string => typeof p === "string") : []);
  } catch {
    return new Set();
  }
}

function saveExpanded(projectId: string, expanded: ReadonlySet<string>): void {
  try {
    window.localStorage.setItem(EXPANDED_KEY(projectId), JSON.stringify([...expanded].slice(0, 500)));
  } catch {
    /* storage unavailable */
  }
}

export interface FileTree {
  dirs: ReadonlyMap<string, DirState>;
  expanded: ReadonlySet<string>;
  /** Loads (or reloads) a directory listing. */
  load: (dir: string) => Promise<void>;
  toggle: (dir: string) => void;
  expand: (dir: string) => void;
  collapseAll: () => void;
  refreshAll: () => void;
  /** Applies a `FileChanged` event. */
  onChange: (change: FileChangedPayload) => void;
}

export function useFileTree(projectId: string, showIgnored: boolean): FileTree {
  const [dirs, setDirs] = useState<Map<string, DirState>>(() => new Map());
  const [expanded, setExpanded] = useState<Set<string>>(() => loadExpanded(projectId));
  const dirsRef = useRef(dirs);
  dirsRef.current = dirs;
  const expandedRef = useRef(expanded);
  expandedRef.current = expanded;
  const pending = useRef(new Set<string>());
  const timer = useRef<number | null>(null);
  const generation = useRef(0);

  const load = useCallback(
    async (dir: string) => {
      const gen = generation.current;
      setDirs((prev) => {
        const next = new Map(prev);
        const cur = prev.get(dir);
        next.set(dir, { status: "loading", entries: cur?.entries });
        return next;
      });
      try {
        const res = await getClient().call("files.list", { projectId, path: dir, showIgnored });
        if (gen !== generation.current) return;
        setDirs((prev) => new Map(prev).set(dir, { status: "loaded", entries: res.entries, truncated: res.truncated }));
      } catch (e) {
        if (gen !== generation.current) return;
        const info = errorInfo(e);
        if (info.code === "not_found" && dir !== "") {
          setDirs((prev) => {
            const next = new Map(prev);
            next.delete(dir);
            return next;
          });
          setExpanded((prev) => {
            const next = new Set(prev);
            next.delete(dir);
            return next;
          });
          return;
        }
        setDirs((prev) => new Map(prev).set(dir, { status: "error", message: info.message, entries: prev.get(dir)?.entries }));
      }
    },
    [projectId, showIgnored],
  );

  // (Re)load the root and every remembered expanded folder.
  useEffect(() => {
    generation.current++;
    setDirs(new Map());
    void load("");
    for (const d of expandedRef.current) void load(d);
  }, [load]);

  useEffect(() => saveExpanded(projectId, expanded), [projectId, expanded]);

  useEffect(
    () => () => {
      if (timer.current !== null) window.clearTimeout(timer.current);
    },
    [],
  );

  const schedule = useCallback(
    (paths: string[]) => {
      for (const p of paths) pending.current.add(p);
      if (timer.current !== null) return;
      timer.current = window.setTimeout(() => {
        timer.current = null;
        const batch = [...pending.current];
        pending.current.clear();
        for (const d of batch) void load(d);
      }, 120);
    },
    [load],
  );

  const toggle = useCallback(
    (dir: string) => {
      const open = expandedRef.current.has(dir);
      setExpanded((prev) => {
        const next = new Set(prev);
        if (open) next.delete(dir);
        else next.add(dir);
        return next;
      });
      if (!open && dirsRef.current.get(dir)?.status !== "loaded") void load(dir);
    },
    [load],
  );

  const expand = useCallback(
    (dir: string) => {
      if (dir === "" || expandedRef.current.has(dir)) {
        if (dir !== "" && dirsRef.current.get(dir)?.status !== "loaded") void load(dir);
        return;
      }
      toggle(dir);
    },
    [toggle, load],
  );

  const onChange = useCallback(
    (change: FileChangedPayload) => {
      setExpanded((prev) => updateExpanded(prev, change));
      const loaded = new Set([...dirsRef.current].filter(([, s]) => s.status !== "error").map(([k]) => k));
      const refresh = dirsToRefresh(change, loaded);
      setDirs((prev) => pruneDirs(prev, change));
      // A renamed folder that was open: load it at its new place.
      if (change.change === "Renamed" && change.oldPath != null && expandedRef.current.has(change.oldPath)) refresh.push(change.path);
      schedule(refresh);
    },
    [schedule],
  );

  return {
    dirs,
    expanded,
    load,
    toggle,
    expand,
    collapseAll: () => setExpanded(new Set()),
    refreshAll: () => schedule(["", ...expandedRef.current]),
    onChange,
  };
}
