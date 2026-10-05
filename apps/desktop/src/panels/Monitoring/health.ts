/**
 * Core health model of the Monitoring panel (§27 local part, §68.45 lite):
 * pure evaluation of service checks, RPC latency statistics and the error
 * log tail (unit-tested), plus the hooks that feed them.
 */
import type { CoreHealthResult, Event, LogLevel, OrkestrProcess } from "@orkestr/protocol";
import { TOPIC_ALL } from "@orkestr/protocol";
import { useEffect, useState } from "react";
import { create } from "zustand";

import { maybeClient } from "../../core/client";
import { describeError } from "../../i18n/errors";
import { useAppStore } from "../../store/app";

/** Result of one health check. */
export type CheckLevel = "ok" | "warn" | "fail" | "unknown";

/** Thresholds of the core service checks (ms / bytes). */
export const THRESHOLDS = {
  rpcWarnMs: 250,
  rpcFailMs: 1500,
  lagWarnMs: 1500,
  lagFailMs: 6000,
  dbWarnBytes: 2 * 1024 ** 3,
  walWarnBytes: 256 * 1024 ** 2,
  objectsWarnBytes: 20 * 1024 ** 3,
} as const;

/** Level of a value against warn/fail thresholds (`null` = not measured). */
export function levelOf(value: number | null | undefined, warn: number, fail = Infinity): CheckLevel {
  if (value === null || value === undefined || !Number.isFinite(value)) return "unknown";
  if (value >= fail) return "fail";
  if (value >= warn) return "warn";
  return "ok";
}

/** Worst of several levels (`unknown` only when everything is unknown). */
export function worstLevel(levels: readonly CheckLevel[]): CheckLevel {
  const rank: Record<CheckLevel, number> = { unknown: 0, ok: 1, warn: 2, fail: 3 };
  let worst: CheckLevel = "unknown";
  for (const l of levels) if (rank[l] > rank[worst]) worst = l;
  return worst;
}

// ── RPC latency ─────────────────────────────────────────────────────────

export interface LatencyStats {
  last: number;
  avg: number;
  p95: number;
  max: number;
  samples: number;
}

/** Statistics over round-trip samples (ms). */
export function latencyStats(samples: readonly number[]): LatencyStats | null {
  const clean = samples.filter((v) => Number.isFinite(v) && v >= 0);
  if (clean.length === 0) return null;
  const sorted = [...clean].sort((a, b) => a - b);
  const sum = clean.reduce((a, b) => a + b, 0);
  const idx = Math.min(sorted.length - 1, Math.ceil(sorted.length * 0.95) - 1);
  return {
    last: clean[clean.length - 1] ?? 0,
    avg: sum / clean.length,
    p95: sorted[Math.max(0, idx)] ?? 0,
    max: sorted[sorted.length - 1] ?? 0,
    samples: clean.length,
  };
}

// ── log tail ────────────────────────────────────────────────────────────

/** One line of the error log tail. */
export interface LogLine {
  id: string;
  seq: number;
  ts: string;
  level: LogLevel;
  target: string;
  message: string;
  projectId: string | null;
}

/** Levels the tail keeps. */
export const TAIL_LEVELS: ReadonlySet<LogLevel> = new Set<LogLevel>(["warn", "error"]);

export const TAIL_MAX = 200;

/** A `Log` event of a kept level as a tail line, else null. */
export function logLineOf(ev: Event): LogLine | null {
  if (ev.kind !== "Log" || !TAIL_LEVELS.has(ev.payload.level)) return null;
  return {
    id: ev.id,
    seq: ev.seq,
    ts: ev.ts,
    level: ev.payload.level,
    target: ev.payload.target,
    message: ev.payload.message,
    projectId: ev.projectId,
  };
}

/** Merges lines into the tail: dedupe by id, newest first, capped. */
export function mergeTail(tail: readonly LogLine[], incoming: readonly LogLine[], max = TAIL_MAX): LogLine[] {
  if (incoming.length === 0) return tail as LogLine[];
  const byId = new Map(tail.map((l) => [l.id, l]));
  for (const l of incoming) byId.set(l.id, l);
  return [...byId.values()]
    .sort((a, b) => (a.seq !== b.seq && a.seq > 0 && b.seq > 0 ? b.seq - a.seq : Date.parse(b.ts) - Date.parse(a.ts)))
    .slice(0, max);
}

// ── processes ───────────────────────────────────────────────────────────

export type ProcessSort = "cpu" | "memory" | "name";

/** Sorted copy of the process rows (running first, then by key). */
export function sortProcesses(rows: readonly OrkestrProcess[], by: ProcessSort): OrkestrProcess[] {
  return [...rows].sort((a, b) => {
    if (a.running !== b.running) return a.running ? -1 : 1;
    if (by === "name") return a.label.localeCompare(b.label);
    if (by === "memory") return (b.memoryBytes ?? -1) - (a.memoryBytes ?? -1);
    return (b.cpuPercent ?? -1) - (a.cpuPercent ?? -1);
  });
}

// ── hooks ───────────────────────────────────────────────────────────────

/** How often the panel probes `dashboard.health`. */
export const HEALTH_POLL_MS = 5000;
/** Round-trip samples kept. */
export const LATENCY_SAMPLES = 60;

export interface CoreHealthState {
  health: CoreHealthResult | null;
  /** Round-trip times of the probes (ms), oldest first. */
  latency: number[];
  error: string | null;
  loading: boolean;
}

/** Polls `dashboard.health` while mounted, timing every round trip. */
export function useCoreHealth(pollMs = HEALTH_POLL_MS): CoreHealthState & { reload: () => void } {
  const connected = useAppStore((s) => s.connection.state === "open");
  const [state, setState] = useState<CoreHealthState>({ health: null, latency: [], error: null, loading: false });
  const [tick, setTick] = useState(0);
  useEffect(() => {
    const client = maybeClient();
    if (!connected || !client) return undefined;
    let cancelled = false;
    let timer: number | undefined;
    const probe = () => {
      const started = performance.now();
      setState((s) => ({ ...s, loading: s.health === null }));
      client.call("dashboard.health", {}).then(
        (health) => {
          if (cancelled) return;
          const rtt = performance.now() - started;
          setState((s) => ({ health, latency: [...s.latency, rtt].slice(-LATENCY_SAMPLES), error: null, loading: false }));
        },
        (e: unknown) => {
          if (!cancelled) setState((s) => ({ ...s, error: describeError(e), loading: false }));
        },
      );
      timer = window.setTimeout(probe, pollMs);
    };
    probe();
    return () => {
      cancelled = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [connected, pollMs, tick]);
  return { ...state, reload: () => setTick((n) => n + 1) };
}

export interface LogTailState {
  lines: LogLine[];
  loaded: boolean;
  ingest: (events: readonly Event[]) => void;
  clear: () => void;
}

export const useLogTail = create<LogTailState>()((set) => ({
  lines: [],
  loaded: false,
  ingest: (events) =>
    set((s) => {
      const lines = events.map(logLineOf).filter((l): l is LogLine => l !== null);
      return { lines: mergeTail(s.lines, lines), loaded: true };
    }),
  clear: () => set({ lines: [] }),
}));

/** Replays recent history once and follows live `Log` events while mounted. */
export function useLogTailFeed(): void {
  const connected = useAppStore((s) => s.connection.state === "open");
  useEffect(() => {
    const client = maybeClient();
    if (!connected || !client) return undefined;
    let stopped = false;
    client.call("events.replay", { limit: 500, newestFirst: true }).then(
      (r) => {
        if (!stopped) useLogTail.getState().ingest(r.events);
      },
      () => {
        if (!stopped) useLogTail.setState({ loaded: true });
      },
    );
    const off = client.subscribe([TOPIC_ALL], (ev) => {
      if (ev.kind === "Log") useLogTail.getState().ingest([ev]);
    });
    return () => {
      stopped = true;
      off();
    };
  }, [connected]);
}
