/**
 * App dependencies (injected so tests can use the in-memory core) and the
 * `useClient(coreId)` hook that connects lazily through `ConnectionManager`.
 */
import type { ConnectionState } from "@orkestr/client";
import { type ReactNode, createContext, useCallback, useContext, useEffect, useState } from "react";

import type { MobileCore, Recorder, RemoteClient, Speaker } from "../core";

export interface Deps {
  core: MobileCore;
  speaker: Speaker;
  /** Recorder hook (expo-audio in the app, fake in tests). */
  useRecorder: () => Recorder;
  /** Native push credentials present in this build. */
  pushConfigured: boolean;
  platform: string;
}

const Ctx = createContext<Deps | null>(null);

export function DepsProvider({ deps, children }: { deps: Deps; children: ReactNode }) {
  return <Ctx.Provider value={deps}>{children}</Ctx.Provider>;
}

export function useDeps(): Deps {
  const d = useContext(Ctx);
  if (!d) throw new Error("DepsProvider missing");
  return d;
}

/** Error code of an RPC/gateway/local failure, for `t("errors.<code>")`. */
export function errorCode(e: unknown): string {
  const c = (e as { code?: unknown } | null)?.code;
  return typeof c === "string" ? c : "internal";
}

export function useClient(coreId: string | undefined): {
  client: RemoteClient | null;
  error: string | null;
  state: ConnectionState | "idle";
  retry: () => void;
} {
  const { core } = useDeps();
  const [client, setClient] = useState<RemoteClient | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [state, setState] = useState<ConnectionState | "idle">("idle");
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    if (!coreId) return;
    let alive = true;
    setError(null);
    core.connections
      .get(coreId)
      .then((c) => {
        if (!alive) return;
        setClient(c);
        const off = (c as { onStateChange?: (l: (s: ConnectionState) => void) => () => void }).onStateChange?.((s) => alive && setState(s));
        if (!off) setState("open");
      })
      .catch((e: unknown) => alive && setError(errorCode(e)));
    return () => {
      alive = false;
    };
  }, [core, coreId, attempt]);

  const retry = useCallback(() => setAttempt((n) => n + 1), []);
  return { client, error, state, retry };
}

/** Runs an async loader with `loading` / `error` state. */
export function useAsync<T>(fn: (() => Promise<T>) | null, deps: unknown[]): { data: T | null; error: string | null; loading: boolean; reload: () => void } {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [n, setN] = useState(0);
  useEffect(() => {
    if (!fn) return;
    let alive = true;
    setLoading(true);
    setError(null);
    fn()
      .then((v) => alive && setData(v))
      .catch((e: unknown) => alive && setError(errorCode(e)))
      .finally(() => alive && setLoading(false));
    return () => {
      alive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [...deps, n, !!fn]);
  return { data, error, loading, reload: () => setN((x) => x + 1) };
}
