/**
 * Live node telemetry shared by Mission Control and the Monitoring panel:
 * a ring buffer of `NodeStatus` samples (`nodes.resources` seeds it, the
 * 2 s heartbeat on `node:<id>` extends it). The subscription is reference
 * counted — it runs only while a consumer is mounted; the buffer survives
 * so charts keep their history when a panel is reopened.
 */
import type { ResourceSample } from "@orkestr/protocol";
import { topic } from "@orkestr/protocol";
import { useEffect } from "react";
import { create } from "zustand";

import { maybeClient } from "../../core/client";
import { describeError } from "../../i18n/errors";
import { useAppStore } from "../../store/app";

/** One point of the time series. */
export interface TelemetryPoint {
  /** Sample time (ms since epoch). */
  ts: number;
  /** Global CPU 0..100. */
  cpu: number;
  /** Per-core CPU 0..100. */
  perCore: number[];
  memUsed: number;
  memTotal: number;
  swapUsed: number;
  swapTotal: number;
  diskUsed: number;
  diskTotal: number;
  /** Orkestr core process CPU 0..100 (of the machine). */
  coreCpu: number | null;
  /** Orkestr core process resident memory. */
  coreMem: number | null;
  /**
   * GPU utilisation 0..100. The node sampler does not measure it yet
   * (INTEGRATOR(P6 nodes): fill from `ResourceSample` once it carries GPU load).
   */
  gpu: number | null;
}

/** 10 minutes at the default 2 s interval. */
export const MAX_POINTS = 300;

export function pointFromSample(s: ResourceSample): TelemetryPoint {
  const ts = Date.parse(s.ts);
  return {
    ts: Number.isFinite(ts) ? ts : Date.now(),
    cpu: clampPct(s.cpuPercent),
    perCore: s.perCorePercent.map(clampPct),
    memUsed: s.memoryUsedBytes,
    memTotal: s.memoryTotalBytes,
    swapUsed: s.swapUsedBytes,
    swapTotal: s.swapTotalBytes,
    diskUsed: Math.max(0, s.diskTotalBytes - s.diskFreeBytes),
    diskTotal: s.diskTotalBytes,
    coreCpu: s.coreProcess ? clampPct(s.coreProcess.cpuPercent) : null,
    coreMem: s.coreProcess ? s.coreProcess.memoryBytes : null,
    gpu: null,
  };
}

export function clampPct(v: number): number {
  return Number.isFinite(v) ? Math.min(100, Math.max(0, v)) : 0;
}

/** Appends a point (dropping duplicates/out-of-order samples) and trims to `max`. */
export function pushPoint(buf: readonly TelemetryPoint[], p: TelemetryPoint, max = MAX_POINTS): TelemetryPoint[] {
  const last = buf[buf.length - 1];
  if (last && p.ts <= last.ts) return buf as TelemetryPoint[];
  const next = buf.length >= max ? buf.slice(buf.length - max + 1) : buf.slice();
  next.push(p);
  return next;
}

/** Points newer than `now - windowMs`. */
export function windowed(points: readonly TelemetryPoint[], windowMs: number, now: number): TelemetryPoint[] {
  const from = now - windowMs;
  return points.filter((p) => p.ts >= from);
}

/** Fraction 0..1 (0 when total is 0). */
export function ratio(used: number, total: number): number {
  return total > 0 ? Math.min(1, Math.max(0, used / total)) : 0;
}

export interface SeriesStats {
  min: number;
  max: number;
  avg: number;
  last: number;
}

export function seriesStats(values: readonly number[]): SeriesStats | null {
  if (values.length === 0) return null;
  let min = Infinity;
  let max = -Infinity;
  let sum = 0;
  for (const v of values) {
    min = Math.min(min, v);
    max = Math.max(max, v);
    sum += v;
  }
  return { min, max, avg: sum / values.length, last: values[values.length - 1] ?? 0 };
}

// ── store ───────────────────────────────────────────────────────────────

export interface TelemetryState {
  points: TelemetryPoint[];
  latest: ResourceSample | null;
  nodeId: string | null;
  intervalMs: number;
  /** Delivery lag of the last heartbeat (receive time − sample time), ms. */
  eventLagMs: number | null;
  error: string | null;
  ingest: (sample: ResourceSample, receivedAt?: number) => void;
}

export const useTelemetryStore = create<TelemetryState>()((set) => ({
  points: [],
  latest: null,
  nodeId: null,
  intervalMs: 2000,
  eventLagMs: null,
  error: null,
  ingest: (sample, receivedAt = Date.now()) =>
    set((s) => {
      const p = pointFromSample(sample);
      return {
        points: pushPoint(s.points, p),
        latest: sample,
        nodeId: sample.nodeId,
        eventLagMs: Math.max(0, receivedAt - p.ts),
        error: null,
      };
    }),
}));

let consumers = 0;
let stop: (() => void) | null = null;

function start(): () => void {
  const client = maybeClient();
  if (!client) return () => {};
  let stopped = false;
  let off: (() => void) | null = null;
  client.call("nodes.resources", {}).then(
    (r) => {
      if (stopped) return;
      useTelemetryStore.setState({ intervalMs: r.intervalMs });
      useTelemetryStore.getState().ingest(r.sample);
      off = client.subscribe([topic.node(r.sample.nodeId)], (ev) => {
        if (ev.kind === "NodeStatus" && ev.payload.sample) useTelemetryStore.getState().ingest(ev.payload.sample);
      });
    },
    (e: unknown) => useTelemetryStore.setState({ error: describeError(e) }),
  );
  return () => {
    stopped = true;
    off?.();
  };
}

/** Starts (or joins) the live stream; returns the release function. */
export function retainTelemetry(): () => void {
  consumers += 1;
  if (consumers === 1) stop = start();
  let released = false;
  return () => {
    if (released) return;
    released = true;
    consumers -= 1;
    if (consumers === 0) {
      stop?.();
      stop = null;
    }
  };
}

/** Keeps the live stream running while the calling component is mounted (and connected). */
export function useTelemetryFeed(): void {
  const connected = useAppStore((s) => s.connection.state === "open");
  useEffect(() => {
    if (!connected) return;
    return retainTelemetry();
  }, [connected]);
}
