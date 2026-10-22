/**
 * Pure helpers of the infrastructure monitor view (ADR-0042). Kept free of
 * React so they are unit-tested in `infra.test.ts`.
 *
 * Wire fields added after the step-0 protocol freeze (`history`,
 * `CheckProbe.Http.method`, `IngestLine.attrs`, `MetricsParams.resolutionSecs`)
 * are typed locally as optional so this view compiles against both the frozen
 * and the regenerated `@orkestr/protocol` types.
 */
import type {
  CheckProbe,
  CheckState,
  HealthCheckStatus,
  LogLevel,
  LogLine,
  MetricSeries,
  NodeMetricsSnapshot,
  UpsertCheckParams,
} from "@orkestr/protocol";

export interface CheckRun {
  ts: string;
  ok: boolean;
  latencyMs?: number;
  error?: string;
}

export type CheckStatusView = HealthCheckStatus & { history?: CheckRun[] };

export type ProbeView = CheckProbe & { method?: string };

export type Tone = "neutral" | "success" | "warning" | "danger" | "info";

export function checkTone(state: CheckState): Tone {
  return state === "Up" ? "success" : state === "Down" ? "danger" : "neutral";
}

export function severityTone(sev: "Info" | "Warning" | "Critical"): Tone {
  return sev === "Critical" ? "danger" : sev === "Warning" ? "warning" : "info";
}

export function levelTone(level: LogLevel | undefined): Tone {
  switch (level) {
    case "error":
      return "danger";
    case "warn":
      return "warning";
    case "info":
      return "info";
    default:
      return "neutral";
  }
}

export function pct(used: number, total: number): number {
  if (!Number.isFinite(used) || !Number.isFinite(total) || total <= 0) return 0;
  return Math.max(0, Math.min(100, (used / total) * 100));
}

export function snapshotPercents(n: NodeMetricsSnapshot): { cpu: number; mem: number; disk: number; gpu: number | null } {
  return {
    cpu: Math.max(0, Math.min(100, n.cpuPercent)),
    mem: pct(n.memUsedBytes, n.memTotalBytes),
    disk: pct(n.diskUsedBytes, n.diskTotalBytes),
    gpu: n.gpuPercent.length ? Math.max(...n.gpuPercent) : null,
  };
}

/** Values of one metric (oldest first). */
export function seriesValues(series: readonly MetricSeries[] | null | undefined, metric: string): number[] {
  return series?.find((s) => s.metric === metric)?.points.map((p) => p.value) ?? [];
}

/** Human summary of a probe (`GET https://…`, `tcp db:5432`). */
export function probeSummary(p: ProbeView): string {
  if (p.type === "Tcp") return `tcp ${p.host}:${p.port}`;
  return `${(p.method ?? "GET").toUpperCase()} ${p.url}`;
}

export interface CheckDraft {
  id?: string;
  name: string;
  kind: "Http" | "Tcp";
  url: string;
  method: string;
  expectStatus: string;
  contains: string;
  host: string;
  port: string;
  intervalSecs: string;
  timeoutMs: string;
  failureThreshold: string;
  enabled: boolean;
}

export const EMPTY_DRAFT: CheckDraft = {
  name: "",
  kind: "Http",
  url: "",
  method: "GET",
  expectStatus: "",
  contains: "",
  host: "",
  port: "",
  intervalSecs: "30",
  timeoutMs: "5000",
  failureThreshold: "2",
  enabled: true,
};

export function draftFromCheck(c: CheckStatusView): CheckDraft {
  const p = c.check.probe as ProbeView;
  return {
    id: c.check.id,
    name: c.check.name,
    kind: p.type,
    url: p.type === "Http" ? p.url : "",
    method: p.type === "Http" ? (p.method ?? "GET") : "GET",
    expectStatus: p.type === "Http" && p.expectStatus != null ? String(p.expectStatus) : "",
    contains: p.type === "Http" ? (p.contains ?? "") : "",
    host: p.type === "Tcp" ? p.host : "",
    port: p.type === "Tcp" ? String(p.port) : "",
    intervalSecs: String(c.check.intervalSecs),
    timeoutMs: String(c.check.timeoutMs),
    failureThreshold: String(c.check.failureThreshold),
    enabled: c.check.enabled,
  };
}

export type DraftError = "name" | "url" | "host" | "port" | "status" | "numbers";

function int(s: string): number | null {
  if (!/^\d+$/.test(s.trim())) return null;
  return Number(s.trim());
}

/** Validates a draft and builds `monitor.checkUpsert` params. */
export function draftToParams(d: CheckDraft): { params: UpsertCheckParams } | { error: DraftError } {
  if (!d.name.trim()) return { error: "name" };
  let probe: ProbeView;
  if (d.kind === "Http") {
    if (!/^https?:\/\/\S+$/i.test(d.url.trim())) return { error: "url" };
    const status = d.expectStatus.trim() ? int(d.expectStatus) : undefined;
    if (status === null || (status !== undefined && (status < 100 || status > 599))) return { error: "status" };
    probe = { type: "Http", url: d.url.trim(), method: d.method.trim().toUpperCase() || "GET" };
    if (status !== undefined) probe.expectStatus = status;
    if (d.contains) probe.contains = d.contains;
  } else {
    if (!d.host.trim()) return { error: "host" };
    const port = int(d.port);
    if (port === null || port < 1 || port > 65535) return { error: "port" };
    probe = { type: "Tcp", host: d.host.trim(), port };
  }
  const interval = int(d.intervalSecs);
  const timeout = int(d.timeoutMs);
  const threshold = int(d.failureThreshold);
  if (interval === null || timeout === null || threshold === null || threshold < 1) return { error: "numbers" };
  const params: UpsertCheckParams = {
    name: d.name.trim(),
    probe,
    intervalSecs: interval,
    timeoutMs: timeout,
    failureThreshold: threshold,
    enabled: d.enabled,
  };
  if (d.id) params.id = d.id;
  return { params };
}

export const MAX_TAIL = 2000;

function lineKey(l: LogLine): string {
  return `${l.ts}|${l.sourceId}|${l.message}`;
}

/** Appends polled lines to the live view: dedups the overlap and caps the buffer. */
export function mergeTail(prev: readonly LogLine[], next: readonly LogLine[], cap = MAX_TAIL): LogLine[] {
  if (next.length === 0) return prev as LogLine[];
  const seen = new Set(prev.slice(-Math.max(next.length * 2, 200)).map(lineKey));
  const out = prev.concat(next.filter((l) => !seen.has(lineKey(l))));
  out.sort((a, b) => Date.parse(a.ts) - Date.parse(b.ts));
  return out.length > cap ? out.slice(out.length - cap) : out;
}

/** Newest timestamp of a buffer (the next poll's `since`). */
export function lastTs(lines: readonly LogLine[]): string | undefined {
  return lines.at(-1)?.ts;
}

/** Up-time ratio of a check's recent runs (null without runs). */
export function upRatio(history: readonly CheckRun[] | undefined): number | null {
  if (!history?.length) return null;
  return history.filter((h) => h.ok).length / history.length;
}
