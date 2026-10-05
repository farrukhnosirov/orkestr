/**
 * Pure helpers of the quality feature (§17, §58): status tones, run
 * aggregation, project-relative paths for jump-to-file and profile edits.
 * No React, no RPC — unit-tested in `quality.test.ts`.
 */
import type {
  CheckDef,
  CheckKind,
  CheckResult,
  QualityCheckStatus,
  GateStatus,
  QualityRun,
  RunTarget,
} from "@orkestr/protocol";
import type { Tone } from "@orkestr/ui";

/** Execution order of check kinds (mirrors `CheckKind::ALL`). */
export const CHECK_KINDS: readonly CheckKind[] = ["Build", "Typecheck", "Lint", "Format", "Test", "E2e", "Smoke"];

/** Kinds counted as tests (mirrors `CheckKind::is_test`). */
export const TEST_KINDS: ReadonlySet<CheckKind> = new Set<CheckKind>(["Test", "E2e", "Smoke"]);

/** Kinds the "Run tests" command runs. */
export const TEST_RUN_KINDS: CheckKind[] = ["Test"];
/** Kinds the "Run build" command runs. */
export const BUILD_RUN_KINDS: CheckKind[] = ["Build"];

/** Whether a check / run status is final. */
export function isFinal(status: QualityCheckStatus): boolean {
  return status !== "Queued" && status !== "Running";
}

/** Badge tone of a check / run status. */
export function checkTone(status: QualityCheckStatus): Tone {
  switch (status) {
    case "Passed":
      return "success";
    case "Failed":
    case "TimedOut":
    case "Error":
      return "danger";
    case "Blocked":
      return "warning";
    case "Running":
    case "Queued":
      return "info";
    default:
      return "neutral";
  }
}

/** Badge tone of a gate status. */
export function gateTone(status: GateStatus): Tone {
  switch (status) {
    case "Passed":
      return "success";
    case "Failed":
      return "danger";
    case "Pending":
      return "info";
    case "Unknown":
      return "warning";
    default:
      return "neutral";
  }
}

/** Status dot (`@orkestr/ui` `Status`) for the status bar indicator. */
export function statusDot(status: QualityCheckStatus | null): "online" | "error" | "busy" | "warning" | "idle" {
  if (status === null) return "idle";
  switch (status) {
    case "Passed":
      return "online";
    case "Failed":
    case "TimedOut":
    case "Error":
      return "error";
    case "Running":
    case "Queued":
      return "busy";
    case "Blocked":
      return "warning";
    default:
      return "idle";
  }
}

/** Totals over a run's results. */
export interface RunTotals {
  passed: number;
  failed: number;
  skipped: number;
  errors: number;
  warnings: number;
  checksPassed: number;
  checksTotal: number;
}

export function runTotals(results: readonly CheckResult[]): RunTotals {
  const totals: RunTotals = { passed: 0, failed: 0, skipped: 0, errors: 0, warnings: 0, checksPassed: 0, checksTotal: results.length };
  for (const r of results) {
    totals.passed += r.passed;
    totals.failed += r.failed;
    totals.skipped += r.skipped;
    for (const d of r.diagnostics) {
      if (d.severity === "Error") totals.errors += 1;
      else if (d.severity === "Warning") totals.warnings += 1;
    }
    if (r.status === "Passed") totals.checksPassed += 1;
  }
  return totals;
}

/** Why a check was skipped / blocked, as a translation key suffix (`reason.<x>`), or null. */
export function resultReason(r: CheckResult): "buildFailed" | "budget" | "approval" | "denied" | "timedOut" | "cancelled" | "error" | null {
  switch (r.status) {
    case "Skipped":
      if (/build failed/i.test(r.summary)) return "buildFailed";
      if (/budget/i.test(r.summary)) return "budget";
      return null;
    case "Blocked":
      return r.approvalId || /approval/i.test(r.summary) ? "approval" : "denied";
    case "TimedOut":
      return "timedOut";
    case "Cancelled":
      return "cancelled";
    case "Error":
      return "error";
    default:
      return null;
  }
}

/** `run:check` build ids published in `Build*` / `TestRunFinished` events → the run id. */
export function runIdOfBuildId(buildId: string): string | null {
  const i = buildId.indexOf(":");
  return i > 0 ? buildId.slice(0, i) : null;
}

function normalize(path: string): string {
  const parts: string[] = [];
  for (const seg of path.replace(/\\/g, "/").split("/")) {
    if (seg === "" || seg === ".") continue;
    if (seg === "..") {
      if (parts.length === 0) return "\0";
      parts.pop();
      continue;
    }
    parts.push(seg);
  }
  return parts.join("/");
}

function isAbsolute(p: string): boolean {
  return /^[a-zA-Z]:[\\/]/.test(p) || p.startsWith("/") || p.startsWith("\\");
}

/**
 * Project-relative (`/`-separated) path of a file reported by a check, for
 * opening it in the Code panel. `file` may be relative to the check's `cwd`
 * (relative to the workspace) or absolute inside `workspace`. Returns null
 * when the file lies outside the workspace.
 */
export function workspaceRelative(file: string, cwd: string, workspace: string): string | null {
  const f = file.trim();
  if (!f) return null;
  if (isAbsolute(f)) {
    const ws = workspace.replace(/\\/g, "/").replace(/\/+$/, "");
    const abs = f.replace(/\\/g, "/");
    const caseless = /^[a-zA-Z]:\//.test(ws);
    const head = caseless ? abs.slice(0, ws.length).toLowerCase() : abs.slice(0, ws.length);
    const base = caseless ? ws.toLowerCase() : ws;
    if (ws && head === base && (abs.length === ws.length || abs[ws.length] === "/")) {
      const rel = normalize(abs.slice(ws.length));
      return rel && rel !== "\0" ? rel : null;
    }
    return null;
  }
  const rel = normalize(`${cwd === "." ? "" : cwd}/${f}`);
  return rel && rel !== "\0" ? rel : null;
}

/** Location to open for a diagnostic / failure, or null. */
export function locationOf(
  item: { file?: string | undefined; line?: number | undefined; column?: number | undefined },
  result: Pick<CheckResult, "cwd">,
  run: Pick<QualityRun, "workspace">,
): { path: string; line: number; column?: number } | null {
  if (!item.file) return null;
  const path = workspaceRelative(item.file, result.cwd, run.workspace);
  if (!path) return null;
  const loc: { path: string; line: number; column?: number } = { path, line: Math.max(1, item.line ?? 1) };
  if (item.column != null) loc.column = item.column;
  return loc;
}

/** Most recent final status per kind across runs (newest first) — used for the status bar. */
export function latestStatus(runs: readonly QualityRun[], kinds: ReadonlySet<CheckKind>): { status: QualityCheckStatus; run: QualityRun } | null {
  for (const run of runs) {
    const relevant = run.results.filter((r) => kinds.has(r.kind));
    if (relevant.length === 0) continue;
    if (relevant.some((r) => r.status === "Running" || r.status === "Queued")) return { status: "Running", run };
    if (relevant.some((r) => r.status === "Failed" || r.status === "TimedOut" || r.status === "Error")) return { status: "Failed", run };
    if (relevant.some((r) => r.status === "Blocked")) return { status: "Blocked", run };
    if (relevant.some((r) => r.status === "Passed")) return { status: "Passed", run };
    return { status: relevant[0]?.status ?? "Skipped", run };
  }
  return null;
}

/** Display form of a run target. */
export function targetKey(target: RunTarget): "project" | "worktree" | "task" {
  return target.type;
}

// ---------------------------------------------------------------------------
// Profile editing
// ---------------------------------------------------------------------------

/** Validation problem of a draft profile (translation key under `settings.errors.*`). */
export type ProfileProblem = { index: number; key: "commandRequired" | "commandMultiline" | "cwdOutside" | "timeout" | "duplicateId" };

const MAX_TIMEOUT_SECS = 60 * 60;

/** Client-side validation mirroring `validate_checks` in the core. */
export function validateProfile(checks: readonly CheckDef[]): ProfileProblem[] {
  const out: ProfileProblem[] = [];
  const seen = new Set<string>();
  checks.forEach((c, index) => {
    if (seen.has(c.id)) out.push({ index, key: "duplicateId" });
    seen.add(c.id);
    const cmd = c.command.trim();
    if (!cmd) out.push({ index, key: "commandRequired" });
    else if (/[\r\n]/.test(cmd)) out.push({ index, key: "commandMultiline" });
    const cwd = c.cwd.trim();
    if (cwd && cwd !== "." && (isAbsolute(cwd) || cwd.includes(":") || normalize(cwd) === "\0" || cwd.split(/[\\/]/).includes(".."))) {
      out.push({ index, key: "cwdOutside" });
    }
    if (c.timeoutSecs != null && (c.timeoutSecs < 1 || c.timeoutSecs > MAX_TIMEOUT_SECS || !Number.isInteger(c.timeoutSecs))) {
      out.push({ index, key: "timeout" });
    }
  });
  return out;
}

/** A unique id for a new user check. */
export function newCheckId(existing: readonly CheckDef[], kind: CheckKind): string {
  const base = `custom-${kind.toLowerCase()}`;
  const ids = new Set(existing.map((c) => c.id));
  if (!ids.has(base)) return base;
  for (let i = 2; ; i += 1) {
    const id = `${base}-${i}`;
    if (!ids.has(id)) return id;
  }
}

/** A blank user check. */
export function blankCheck(existing: readonly CheckDef[], kind: CheckKind = "Test"): CheckDef {
  return {
    id: newCheckId(existing, kind),
    kind,
    toolchain: "Custom",
    label: "",
    command: "",
    cwd: ".",
    enabled: true,
    source: "User",
  };
}

/** Kinds with at least one enabled check. */
export function enabledKinds(checks: readonly CheckDef[]): Set<CheckKind> {
  return new Set(checks.filter((c) => c.enabled).map((c) => c.kind));
}
