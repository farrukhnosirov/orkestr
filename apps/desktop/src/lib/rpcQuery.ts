/**
 * Small data hook for panels: runs an RPC loader against the core client,
 * tracks loading / error state, and reloads when matching events arrive.
 * All business data still flows through `@orkestr/client` (contract §8).
 */
import type { OrkestrClient } from "@orkestr/client";
import { OrkestrError } from "@orkestr/client";
import type { Event, EventKindName } from "@orkestr/protocol";
import { useCallback, useEffect, useRef, useState } from "react";

import { maybeClient } from "../core/client";
import { useAppStore } from "../store/app";

export interface RpcQuery<T> {
  data: T | null;
  error: string | null;
  loading: boolean;
  reload: () => void;
}

export interface RpcQueryOptions {
  /** Topics to subscribe to (e.g. `global`, `project:<id>`). */
  topics?: string[];
  /** Event kinds that trigger a reload. */
  reloadOn?: readonly EventKindName[];
  /** Skip loading entirely (e.g. no project selected). */
  enabled?: boolean;
}

/** Human-readable message for any thrown value. */
export function errorMessage(e: unknown): string {
  if (e instanceof OrkestrError) return e.message;
  if (e instanceof Error) return e.message;
  return String(e);
}

/**
 * `key` identifies the query: when it changes, data is reloaded.
 * Event-triggered reloads are debounced (150 ms) to coalesce bursts.
 */
export function useRpcQuery<T>(
  key: string,
  loader: (client: OrkestrClient) => Promise<T>,
  options: RpcQueryOptions = {},
): RpcQuery<T> {
  const connected = useAppStore((s) => s.connection.state === "open");
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [tick, setTick] = useState(0);
  const loaderRef = useRef(loader);
  loaderRef.current = loader;
  const enabled = options.enabled ?? true;
  const topicsKey = (options.topics ?? []).join("|");
  const reloadKey = (options.reloadOn ?? []).join("|");

  const reload = useCallback(() => setTick((t) => t + 1), []);

  useEffect(() => {
    if (!enabled) {
      setData(null);
      setError(null);
      setLoading(false);
      return;
    }
    const client = maybeClient();
    if (!connected || !client) return;
    let cancelled = false;
    setLoading(true);
    loaderRef
      .current(client)
      .then((res) => {
        if (cancelled) return;
        setData(res);
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
  }, [key, tick, connected, enabled]);

  useEffect(() => {
    const client = maybeClient();
    const topics = topicsKey ? topicsKey.split("|") : [];
    const kinds = new Set(reloadKey ? reloadKey.split("|") : []);
    if (!connected || !client || !enabled || topics.length === 0 || kinds.size === 0) return;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const off = client.subscribe(topics, (ev: Event) => {
      if (!kinds.has(ev.kind)) return;
      if (timer) clearTimeout(timer);
      timer = setTimeout(() => setTick((t) => t + 1), 150);
    });
    return () => {
      if (timer) clearTimeout(timer);
      off();
    };
  }, [topicsKey, reloadKey, connected, enabled]);

  return { data, error, loading, reload };
}

/** The connected client, or an error suitable for UI display. */
export function requireClient(): OrkestrClient {
  const c = maybeClient();
  if (!c) throw new OrkestrError("disconnected", "Not connected to the Orkestr core.");
  return c;
}
