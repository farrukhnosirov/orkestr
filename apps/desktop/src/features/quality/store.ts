/**
 * Quality data (§17, §58) over `quality.*` RPC. Runs are kept per project
 * (newest first) and refreshed live from `project:<id>` events
 * (`BuildStarted`, `BuildSucceeded`, `BuildFailed`, `TestRunFinished` carry
 * `<runId>:<checkId>` build ids → `quality.getRun`).
 */
import type { CheckDef, CheckKind, DodSetting, Event, GateReport, GateSetting, QualityRun, RunTarget } from "@orkestr/protocol";
import { topic } from "@orkestr/protocol";
import { useEffect } from "react";
import { create } from "zustand";

import { getClient, maybeClient } from "../../core/client";
import { describeError } from "../../i18n/errors";
import { useAppStore } from "../../store/app";
import { runIdOfBuildId } from "./logic";

const QUALITY_EVENTS: ReadonlySet<string> = new Set(["BuildStarted", "BuildSucceeded", "BuildFailed", "TestRunFinished"]);

/** Max runs kept per project in memory. */
const KEEP_RUNS = 50;

interface ProjectRuns {
  runs: QualityRun[];
  loading: boolean;
  error: string | null;
  loaded: boolean;
}

export interface QualityState {
  byProject: Record<string, ProjectRuns>;
  /** Bumped whenever a run changes (consumers re-evaluate gates). */
  version: number;
  load: (projectId: string) => Promise<void>;
  upsertRun: (run: QualityRun) => void;
  refreshRun: (runId: string) => Promise<void>;
}

const EMPTY: ProjectRuns = { runs: [], loading: false, error: null, loaded: false };

/** Inserts or replaces a run, newest first. */
export function mergeRun(runs: readonly QualityRun[], run: QualityRun): QualityRun[] {
  const next = runs.filter((r) => r.id !== run.id);
  next.push(run);
  next.sort((a, b) => (a.startedAt < b.startedAt ? 1 : a.startedAt > b.startedAt ? -1 : 0));
  return next.slice(0, KEEP_RUNS);
}

export const useQualityStore = create<QualityState>()((set, get) => ({
  byProject: {},
  version: 0,
  load: async (projectId) => {
    const client = maybeClient();
    if (!client) return;
    set((s) => ({ byProject: { ...s.byProject, [projectId]: { ...(s.byProject[projectId] ?? EMPTY), loading: true, error: null } } }));
    try {
      const { runs } = await client.call("quality.runs", { projectId, taskId: null, limit: KEEP_RUNS });
      set((s) => ({
        byProject: { ...s.byProject, [projectId]: { runs, loading: false, error: null, loaded: true } },
        version: s.version + 1,
      }));
    } catch (e) {
      set((s) => ({
        byProject: { ...s.byProject, [projectId]: { ...(s.byProject[projectId] ?? EMPTY), loading: false, error: describeError(e) } },
      }));
    }
  },
  upsertRun: (run) =>
    set((s) => {
      const current = s.byProject[run.projectId] ?? EMPTY;
      return {
        byProject: { ...s.byProject, [run.projectId]: { ...current, runs: mergeRun(current.runs, run) } },
        version: s.version + 1,
      };
    }),
  refreshRun: async (runId) => {
    const client = maybeClient();
    if (!client) return;
    try {
      get().upsertRun(await client.call("quality.getRun", { runId }));
    } catch (e) {
      console.warn("quality.getRun failed", e);
    }
  },
}));

/** Applies a project event; returns the run id to refresh, if any. */
export function runIdOfEvent(ev: Event): string | null {
  if (!QUALITY_EVENTS.has(ev.kind)) return null;
  switch (ev.kind) {
    case "BuildStarted":
    case "BuildSucceeded":
    case "BuildFailed":
      return runIdOfBuildId(ev.payload.buildId);
    case "TestRunFinished":
      return runIdOfBuildId(ev.payload.runId);
    default:
      return null;
  }
}

/** Loads a project's runs and keeps them live while mounted. */
export function useQualityLive(projectId: string | null): ProjectRuns {
  const connected = useAppStore((s) => s.connection.state === "open");
  const entry = useQualityStore((s) => (projectId ? s.byProject[projectId] : undefined)) ?? EMPTY;
  useEffect(() => {
    if (!projectId || !connected) return;
    const client = maybeClient();
    if (!client) return;
    void useQualityStore.getState().load(projectId);
    const pending = new Set<string>();
    let timer: number | null = null;
    const off = client.subscribe([topic.project(projectId)], (ev) => {
      const runId = runIdOfEvent(ev);
      if (!runId) return;
      pending.add(runId);
      if (timer !== null) return;
      timer = window.setTimeout(() => {
        timer = null;
        const ids = [...pending];
        pending.clear();
        for (const id of ids) void useQualityStore.getState().refreshRun(id);
      }, 150);
    });
    return () => {
      off();
      if (timer !== null) window.clearTimeout(timer);
    };
  }, [projectId, connected]);
  return entry;
}

/** Starts checks (returns the initial run state). */
export async function startRun(projectId: string, opts: { kinds?: CheckKind[]; target?: RunTarget; checkIds?: string[] } = {}): Promise<QualityRun> {
  const run = await getClient().call("quality.run", {
    projectId,
    kinds: opts.kinds ?? null,
    checkIds: opts.checkIds ?? null,
    target: opts.target ?? null,
    agentId: null,
    budgetSecs: null,
    wait: null,
  });
  useQualityStore.getState().upsertRun(run);
  return run;
}

/** Cancels an active run. */
export async function cancelRun(runId: string): Promise<boolean> {
  const r = await getClient().call("quality.cancel", { runId });
  return r.cancelled;
}

/** Evaluates gates + DoD of a task (or the project). */
export function evaluate(projectId: string, taskId: string | null): Promise<GateReport> {
  return getClient().call("quality.evaluate", { projectId, taskId });
}

export const qualityApi = {
  detect: (projectId: string) => getClient().call("quality.detect", { projectId }),
  getProfile: (projectId: string) => getClient().call("quality.getProfile", { projectId }),
  setProfile: (projectId: string, checks: CheckDef[], reset = false) =>
    getClient().call("quality.setProfile", { projectId, checks, reset }),
  gates: (projectId: string) => getClient().call("quality.gates", { projectId }),
  setGates: (projectId: string, patch: { gates?: GateSetting[]; dod?: DodSetting[]; reset?: boolean }) =>
    getClient().call("quality.setGates", { projectId, gates: patch.gates ?? [], dod: patch.dod ?? [], reset: patch.reset ?? false }),
};
