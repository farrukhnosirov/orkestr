/**
 * Pure helpers of the alert-rules editor and the log-sources manager
 * (ADR-0042). Validation mirrors `orkestr_monitor::alerts::validate_rule` and
 * `MonitorService::add_file_source` so most mistakes are caught before the
 * round trip; the core stays the authority.
 */
import type { AddFileSourceParams, AlertRule, AlertSeverity, LogLevel, LogSourceInfo } from "@orkestr/protocol";

export type RuleKindName = "CheckDown" | "MetricThreshold" | "LogRate";
export const RULE_KINDS: readonly RuleKindName[] = ["CheckDown", "MetricThreshold", "LogRate"];
export const SEVERITIES: readonly AlertSeverity[] = ["Info", "Warning", "Critical"];
export const RULE_METRICS = ["cpu", "mem", "disk", "gpu0"] as const;

/** Form state: every number is kept as typed text. */
export interface RuleDraft {
  /** The id of the rule being edited (`null` = new). */
  editing: string | null;
  id: string;
  name: string;
  severity: AlertSeverity;
  enabled: boolean;
  projectId: string;
  kind: RuleKindName;
  checkId: string;
  metric: string;
  nodeId: string;
  above: string;
  samples: string;
  sourceId: string;
  minLevel: LogLevel | "";
  pattern: string;
  count: string;
  windowSecs: string;
}

export const EMPTY_RULE: RuleDraft = {
  editing: null,
  id: "",
  name: "",
  severity: "Warning",
  enabled: true,
  projectId: "",
  kind: "MetricThreshold",
  checkId: "",
  metric: "cpu",
  nodeId: "",
  above: "90",
  samples: "6",
  sourceId: "",
  minLevel: "error",
  pattern: "",
  count: "10",
  windowSecs: "60",
};

export type RuleDraftError = "id" | "name" | "metric" | "above" | "samples" | "count" | "window" | "pattern";

const ID_RE = /^[A-Za-z0-9._-]{1,64}$/;
const METRIC_RE = /^(cpu|mem|disk|gpu\d{1,2})$/;

/** Suggests an id from a name (`"CPU hot!"` → `cpu-hot`). */
export function slugId(name: string): string {
  return name
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 64);
}

export function draftFromRule(r: AlertRule): RuleDraft {
  const d: RuleDraft = { ...EMPTY_RULE, editing: r.id, id: r.id, name: r.name, severity: r.severity, enabled: r.enabled, projectId: r.projectId ?? "", kind: r.kind.type };
  switch (r.kind.type) {
    case "CheckDown":
      return { ...d, checkId: r.kind.checkId ?? "" };
    case "MetricThreshold":
      return { ...d, metric: r.kind.metric, nodeId: r.kind.nodeId ?? "", above: String(r.kind.above), samples: String(r.kind.samples) };
    case "LogRate":
      return {
        ...d,
        sourceId: r.kind.sourceId ?? "",
        minLevel: r.kind.minLevel ?? "",
        pattern: r.kind.pattern ?? "",
        count: String(r.kind.count),
        windowSecs: String(r.kind.windowSecs),
      };
  }
}

function int(s: string, min: number, max: number): number | null {
  if (!/^\d+$/.test(s.trim())) return null;
  const n = Number(s.trim());
  return n >= min && n <= max ? n : null;
}

/** Builds the wire rule or names the first invalid field. */
export function ruleDraftToRule(d: RuleDraft): { rule: AlertRule } | { error: RuleDraftError } {
  const name = d.name.trim();
  const id = (d.editing ?? d.id.trim()) || slugId(name);
  if (!name || name.length > 120) return { error: "name" };
  if (!ID_RE.test(id)) return { error: "id" };
  const base = { id, name, severity: d.severity, enabled: d.enabled, ...(d.projectId ? { projectId: d.projectId } : {}) };
  switch (d.kind) {
    case "CheckDown":
      return { rule: { ...base, kind: { type: "CheckDown", ...(d.checkId ? { checkId: d.checkId } : {}) } } };
    case "MetricThreshold": {
      const metric = d.metric.trim();
      if (!METRIC_RE.test(metric)) return { error: "metric" };
      const above = Number(d.above.trim());
      if (d.above.trim() === "" || !Number.isFinite(above) || above < 0 || above >= 100) return { error: "above" };
      const samples = int(d.samples, 1, 1000);
      if (samples == null) return { error: "samples" };
      return { rule: { ...base, kind: { type: "MetricThreshold", metric, above, samples, ...(d.nodeId ? { nodeId: d.nodeId } : {}) } } };
    }
    case "LogRate": {
      const count = int(d.count, 1, 100_000);
      if (count == null) return { error: "count" };
      const windowSecs = int(d.windowSecs, 1, 86_400);
      if (windowSecs == null) return { error: "window" };
      const pattern = d.pattern.trim();
      if (pattern.length > 200) return { error: "pattern" };
      return {
        rule: {
          ...base,
          kind: {
            type: "LogRate",
            count,
            windowSecs,
            ...(d.sourceId ? { sourceId: d.sourceId } : {}),
            ...(d.minLevel ? { minLevel: d.minLevel } : {}),
            ...(pattern ? { pattern } : {}),
          },
        },
      };
    }
  }
}

/** Absolute path on Windows (`C:\`, `\\server\share`) or POSIX (`/`). */
export function isAbsolutePath(p: string): boolean {
  return /^[A-Za-z]:[\\/]/.test(p) || /^\\\\[^\\.?]/.test(p) || p.startsWith("/");
}

export type SourceDraftError = "path" | "name" | "duplicate";

export function sourceDraftToParams(
  name: string,
  path: string,
  existing: readonly LogSourceInfo[],
): { params: AddFileSourceParams } | { error: SourceDraftError } {
  const p = path.trim();
  if (!p || !isAbsolutePath(p)) return { error: "path" };
  const n = name.trim();
  if (n.length > 200) return { error: "name" };
  const label = (n || p).toLowerCase();
  if (existing.some((s) => s.name.toLowerCase() === label || (s.kind.type === "File" && s.kind.path.toLowerCase() === p.toLowerCase())))
    return { error: "duplicate" };
  return { params: { path: p, ...(n ? { name: n } : {}) } };
}

/** Only tailed files are user-managed; ingest streams and deployment logs are built in. */
export function isRemovableSource(s: LogSourceInfo): boolean {
  return s.kind.type === "File";
}

/** Short technical description of a source (`path`, `stream`, `deployment`). */
export function sourceDetail(s: LogSourceInfo): string {
  switch (s.kind.type) {
    case "File":
      return s.kind.path;
    case "Ingest":
      return s.kind.stream;
    case "Deployment":
      return s.kind.deploymentId;
  }
}
