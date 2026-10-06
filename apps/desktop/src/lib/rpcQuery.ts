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
import { describeError } from "../i18n/errors";
import { t } from "../i18n";
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

/** Human-readable, localized message for any thrown value (see `i18n/errors.ts`). */
export function errorMessage(e: unknown): string {
  return describeError(e);
}

/** What a load produced, tagged with the key it was loaded for. */
interface Loaded<T> {
  key: string;
  data: T | null;
  error: string | null;
}

/**
 * `key` identifies the query: when it changes, data is reloaded and the
 * previous key's data/error are no longer returned (callers never see one
 * project's data under another project's key while the new load runs or
 * after it fails). Event-triggered reloads of the same key keep the data.
 * Event-triggered reloads are debounced (150 ms) to coalesce bursts.
 */
export function useRpcQuery<T>(
  key: string,
  loader: (client: OrkestrClient) => Promise<T>,
  options: RpcQueryOptions = {},
): RpcQuery<T> {
  const connected = useAppStore((s) => s.connection.state === "open");
  const [loaded, setLoaded] = useState<Loaded<T>>({ key, data: null, error: null });
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
      setLoaded({ key, data: null, error: null });
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
        setLoaded({ key, data: res, error: null });
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        // A failed reload of the same key keeps its last good data.
        const message = errorMessage(e);
        setLoaded((prev) => ({ key, data: prev.key === key ? prev.data : null, error: message }));
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

  const current = loaded.key === key;
  return { data: current ? loaded.data : null, error: current ? loaded.error : null, loading, reload };
}

/** The connected client, or an error suitable for UI display. */
export function requireClient(): OrkestrClient {
  const c = maybeClient();
  if (!c) throw new OrkestrError("disconnected", t("common:errors.notConnected"));
  return c;
}
