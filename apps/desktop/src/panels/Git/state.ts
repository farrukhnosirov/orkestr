/**
 * Git panel view state + small data hooks. Business data always comes from
 * the core over RPC (`@orkestr/client`); this file only keeps UI state.
 */
import { OrkestrError } from "@orkestr/client";
import type { Event, EventKindName } from "@orkestr/protocol";
import { topic } from "@orkestr/protocol";
import { type DependencyList, useCallback, useEffect, useRef, useState } from "react";
import { create } from "zustand";

import { maybeClient } from "../../core/client";
import { describeError } from "../../i18n/errors";
import { useAppStore } from "../../store/app";

export type GitTab = "changes" | "branches" | "worktrees" | "history" | "snapshots" | "merge";

/** Requests coming from commands (palette/shortcuts) to the panel. */
export type GitIntent = "commit" | "rollback" | null;

interface GitUiState {
  tab: GitTab;
  /** Active worktree path per project (null = project root). */
  worktrees: Record<string, string | null>;
  intent: GitIntent;
  setTab: (tab: GitTab) => void;
  setWorktree: (projectId: string, path: string | null) => void;
  setIntent: (intent: GitIntent) => void;
}

export const useGitUi = create<GitUiState>()((set) => ({
  tab: "changes",
  worktrees: {},
  intent: null,
  setTab: (tab) => set({ tab }),
  setWorktree: (projectId, path) => set((s) => ({ worktrees: { ...s.worktrees, [projectId]: path } })),
  setIntent: (intent) => set({ intent }),
}));

/** User-facing message of an RPC/client error (localized code label + core detail). */
export function errorMessage(e: unknown): string {
  return describeError(e);
}

/** The core's raw (English, untranslated) message — for matching, never for display. */
export function rawMessage(e: unknown): string {
  if (e instanceof Error) return e.message;
  return String(e);
}

/** Error code of an RPC error (`approval_required`, `conflict`, …). */
export function errorCode(e: unknown): string | null {
  return e instanceof OrkestrError ? e.code : null;
}

export interface AsyncState<T> {
  data: T | null;
  error: string | null;
  loading: boolean;
  reload: () => void;
}

/**
 * Runs `fn` when deps change (and on `reload()`); keeps the previous data
 * while reloading so lists do not flicker.
 */
export function useAsync<T>(fn: () => Promise<T>, deps: DependencyList): AsyncState<T> {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [tick, setTick] = useState(0);
  const fnRef = useRef(fn);
  fnRef.current = fn;
  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    fnRef
      .current()
      .then((d) => {
        if (cancelled) return;
        setData(d);
        setError(null);
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(errorMessage(e));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [...deps, tick]);
  const reload = useCallback(() => setTick((t) => t + 1), []);
  return { data, error, loading, reload };
}

const REFRESH_KINDS: ReadonlySet<EventKindName> = new Set<EventKindName>([
  "FileChanged",
  "SnapshotCreated",
  "RollbackPerformed",
]);

/**
 * A counter that increases (debounced) whenever the project's files or
 * snapshots change, so views can refresh themselves.
 */
export function useProjectRefresh(projectId: string | null): number {
  const connected = useAppStore((s) => s.connection.state === "open");
  const [version, setVersion] = useState(0);
  useEffect(() => {
    if (!projectId || !connected) return;
    const client = maybeClient();
    if (!client) return;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const off = client.subscribe([topic.project(projectId)], (ev: Event) => {
      if (!REFRESH_KINDS.has(ev.kind)) return;
      if (timer) clearTimeout(timer);
      timer = setTimeout(() => setVersion((v) => v + 1), 250);
    });
    const onFocus = () => setVersion((v) => v + 1);
    window.addEventListener("focus", onFocus);
    return () => {
      off();
      if (timer) clearTimeout(timer);
      window.removeEventListener("focus", onFocus);
    };
  }, [projectId, connected]);
  return version;
}
