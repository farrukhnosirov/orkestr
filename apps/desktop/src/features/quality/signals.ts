/**
 * Phase 2 quality signals for the Visual OS (Phase 3): one place that turns
 * quality-runner runs (`quality.runs`), reviews (`reviews.list`) and merge
 * proposals (`merge.list`) into the small views Mission Control, the HUD,
 * the voice assistant and the robots show. Pure mappings + thin RPC helpers;
 * live updates come from the quality store (`useQualityLive`).
 */
import type { CheckKind, CheckResult, MergeManagerProposal, QualityRun, Review } from "@orkestr/protocol";

import { getClient } from "../../core/client";
import { TEST_KINDS } from "./logic";

/** Kinds that count as "the build" (compile / typecheck). */
export const BUILD_KINDS: ReadonlySet<CheckKind> = new Set<CheckKind>(["Build", "Typecheck"]);

const FAILED: ReadonlySet<string> = new Set(["Failed", "TimedOut", "Error"]);
const ACTIVE: ReadonlySet<string> = new Set(["Running", "Queued"]);

/** Results of a run for a set of kinds. */
function resultsOf(run: QualityRun, kinds: ReadonlySet<CheckKind>): CheckResult[] {
  return run.results.filter((r) => kinds.has(r.kind));
}

// ── Mission Control rows ─────────────────────────────────────────────────

/** A Mission Control quality row (`panels/MissionControl/feed.ts` contract). */
export interface MissionQualityRow {
  id: string;
  kind: "build" | "test";
  status: "running" | "passed" | "failed";
  projectId: string | null;
  label: string;
  startedAt: string;
  finishedAt: string | null;
  durationMs: number | null;
  passed: number | null;
  failed: number | null;
  skipped: number | null;
  seq: number;
}

function rowStatus(r: CheckResult): MissionQualityRow["status"] | null {
  if (ACTIVE.has(r.status)) return "running";
  if (FAILED.has(r.status)) return "failed";
  if (r.status === "Passed") return "passed";
  return null;
}

/**
 * One row per build / test check of each run, ids `<runId>:<checkId>` — the
 * same ids the runner's `Build*` / `TestRunFinished` events carry, so rows
 * from RPC and from live events merge.
 */
export function missionRowsOf(runs: readonly QualityRun[]): MissionQualityRow[] {
  const out: MissionQualityRow[] = [];
  for (const run of runs) {
    for (const r of run.results) {
      const test = TEST_KINDS.has(r.kind);
      if (!test && !BUILD_KINDS.has(r.kind)) continue;
      const status = rowStatus(r);
      if (!status) continue;
      out.push({
        id: `${run.id}:${r.checkId}`,
        kind: test ? "test" : "build",
        status,
        projectId: run.projectId,
        label: r.label || r.command,
        startedAt: r.startedAt ?? run.startedAt,
        finishedAt: status === "running" ? null : (run.finishedAt ?? null),
        durationMs: status === "running" ? null : r.durationMs,
        passed: test ? r.passed : null,
        failed: test ? r.failed : null,
        skipped: test ? r.skipped : null,
        seq: 0,
      });
    }
  }
  return out;
}

/** Merges rows (event-derived rows win for the same id: they are newer), newest first. */
export function mergeMissionRows<T extends { id: string; startedAt: string }>(live: readonly T[], loaded: readonly T[], max = 20): T[] {
  const byId = new Map<string, T>();
  for (const r of loaded) byId.set(r.id, r);
  for (const r of live) byId.set(r.id, r);
  return [...byId.values()].sort((a, b) => b.startedAt.localeCompare(a.startedAt)).slice(0, max);
}

// ── HUD / voice snapshot ─────────────────────────────────────────────────

/** Latest build / test facts of a project. */
export interface QualityFacts {
  lastBuild: { ok: boolean; at: string; summary: string } | null;
  lastTests: { passed: number; failed: number; at: string } | null;
  running: boolean;
}

/** Newest finished build and test results (runs newest first). */
export function qualityFactsOf(runs: readonly QualityRun[]): QualityFacts {
  let lastBuild: QualityFacts["lastBuild"] = null;
  let lastTests: QualityFacts["lastTests"] = null;
  let running = false;
  for (const run of runs) {
    if (run.results.some((r) => ACTIVE.has(r.status))) running = true;
    const at = run.finishedAt ?? run.startedAt;
    if (!lastBuild) {
      const builds = resultsOf(run, BUILD_KINDS).filter((r) => !ACTIVE.has(r.status) && r.status !== "Skipped" && r.status !== "Cancelled");
      if (builds.length > 0) {
        const failed = builds.find((r) => FAILED.has(r.status) || r.status === "Blocked");
        lastBuild = { ok: !failed, at, summary: (failed ?? builds[0])?.summary ?? "" };
      }
    }
    if (!lastTests) {
      const tests = resultsOf(run, TEST_KINDS).filter((r) => !ACTIVE.has(r.status) && r.status !== "Skipped" && r.status !== "Cancelled");
      if (tests.length > 0) {
        lastTests = {
          passed: tests.reduce((n, r) => n + r.passed, 0),
          failed: tests.reduce((n, r) => n + r.failed + (FAILED.has(r.status) && r.failed === 0 ? 1 : 0), 0),
          at,
        };
      }
    }
    if (lastBuild && lastTests) break;
  }
  return { lastBuild, lastTests, running };
}

/** HUD chip state from the facts (`null` = nothing to show). */
export function hudStateOf(f: QualityFacts): "passing" | "failing" | "running" | null {
  if (f.running) return "running";
  if (f.lastBuild && !f.lastBuild.ok) return "failing";
  if (f.lastTests && f.lastTests.failed > 0) return "failing";
  if (f.lastBuild || f.lastTests) return "passing";
  return null;
}

/** A failed build-ish check, newest first (voice "show build errors"). */
export interface QualityBuildError {
  at: string;
  summary: string;
  exitCode: number | null;
}

export function buildErrorsOf(runs: readonly QualityRun[]): QualityBuildError[] {
  const out: QualityBuildError[] = [];
  for (const run of runs) {
    for (const r of resultsOf(run, BUILD_KINDS)) {
      if (!FAILED.has(r.status)) continue;
      const firstError = r.diagnostics.find((d) => d.severity === "Error");
      const where = firstError?.file ? `${firstError.file}${firstError.line ? `:${firstError.line}` : ""}: ` : "";
      out.push({
        at: run.finishedAt ?? run.startedAt,
        summary: firstError ? `${where}${firstError.message}` : r.summary,
        exitCode: r.exitCode ?? null,
      });
    }
  }
  return out.sort((a, b) => b.at.localeCompare(a.at));
}

// ── Reviews + merges ─────────────────────────────────────────────────────

/** Reviews still open: running / verifying, or finished asking for changes with unresolved findings. */
export function openReviewCount(reviews: readonly Review[]): number {
  return reviews.filter(
    (r) =>
      r.status === "Running" ||
      r.status === "Verifying" ||
      (r.status === "Finished" && r.verdict !== null && r.verdict !== "Approve" && r.findings.some((f) => f.status === "Open" || f.status === "Confirmed")),
  ).length;
}

const MERGE_ACTIVE: ReadonlySet<string> = new Set(["Proposed", "AwaitingApproval", "Merging", "Conflicted", "Testing"]);

/** Merge proposals that still need attention or are in flight. */
export function activeMerges(proposals: readonly MergeManagerProposal[]): MergeManagerProposal[] {
  return proposals.filter((p) => MERGE_ACTIVE.has(p.status));
}

// ── RPC helpers ──────────────────────────────────────────────────────────

export async function fetchRuns(projectId: string, limit = 30): Promise<QualityRun[]> {
  return (await getClient().call("quality.runs", { projectId, limit })).runs;
}

export async function fetchReviews(projectId: string, limit = 50): Promise<Review[]> {
  return (await getClient().call("reviews.list", { projectId, limit })).reviews;
}

export async function fetchMerges(projectId: string, limit = 10): Promise<MergeManagerProposal[]> {
  return (await getClient().call("merge.list", { projectId, limit })).proposals;
}
