/**
 * Real `VoiceDeps`: RPC through `@orkestr/client`, UI actions through the
 * workspace bridge, settings, the security store and the command registry.
 */
import { topic } from "@orkestr/protocol";

import { getClient, openProject } from "../../../core/client";
import { currentLanguage } from "../../../i18n";
import { workspaceActions } from "../../../components/workspaceActions";
import { useAppStore } from "../../../store/app";
import { stopAllAi } from "../../commands/coreCommands";
import { resolveApproval } from "../../security/store";
import { useSecurityStore } from "../../security/store";
import { useSettingsStore } from "../../settings/settingsStore";
import { startRun } from "../../quality/store";
import { buildErrorsOf, fetchReviews, fetchRuns, openReviewCount, qualityFactsOf } from "../../quality/signals";
import { useVoiceStore } from "../store";
import type { BuildError, VoiceDeps } from "./executor";
import { PHRASE_TIMEOUT_MS } from "./natural";
import { EMPTY_QUALITY, type QualitySnapshot } from "./summary";

/** Recent `BuildFailed` events of a project, newest first (Phase 1 event log). */
async function buildErrorsFromLog(projectId: string): Promise<BuildError[]> {
  const r = await getClient().call("events.replay", { topics: [topic.project(projectId)], newestFirst: true, limit: 500 });
  const out: BuildError[] = [];
  for (const ev of r.events) {
    if (ev.kind === "BuildFailed") out.push({ at: ev.ts, summary: ev.payload.summary, exitCode: ev.payload.exitCode });
  }
  return out.sort((a, b) => b.at.localeCompare(a.at));
}

/**
 * Build failures: the quality runner's failed build / typecheck checks (with
 * the first diagnostic), falling back to `BuildFailed` events of the log.
 */
async function buildErrors(projectId: string): Promise<BuildError[]> {
  try {
    const errors = buildErrorsOf(await fetchRuns(projectId));
    if (errors.length > 0) return errors;
  } catch (e) {
    console.debug("quality.runs unavailable; using the event log", e);
  }
  return buildErrorsFromLog(projectId);
}

/** Phase 2 quality: latest build / tests from the quality runner + open reviews. */
async function quality(projectId: string): Promise<QualitySnapshot> {
  const [runs, reviews] = await Promise.all([fetchRuns(projectId).catch(() => null), fetchReviews(projectId).catch(() => null)]);
  const facts = runs ? qualityFactsOf(runs) : null;
  const fromLog = !facts || (!facts.lastBuild && !facts.lastTests) ? await qualityFromLog(projectId) : EMPTY_QUALITY;
  return {
    lastBuild: facts?.lastBuild ?? fromLog.lastBuild,
    lastTests: facts?.lastTests ?? fromLog.lastTests,
    openReviews: reviews ? openReviewCount(reviews) : null,
  };
}

async function qualityFromLog(projectId: string): Promise<QualitySnapshot> {
  const r = await getClient().call("events.replay", { topics: [topic.project(projectId)], newestFirst: true, limit: 300 });
  let lastBuild: QualitySnapshot["lastBuild"] = null;
  let lastTests: QualitySnapshot["lastTests"] = null;
  for (const ev of [...r.events].reverse()) {
    if (!lastBuild && (ev.kind === "BuildFailed" || ev.kind === "BuildSucceeded")) {
      lastBuild = { ok: ev.kind === "BuildSucceeded", at: ev.ts, summary: ev.payload.summary };
    }
    if (!lastTests && ev.kind === "TestRunFinished") lastTests = { passed: ev.payload.passed, failed: ev.payload.failed, at: ev.ts };
  }
  return { ...EMPTY_QUALITY, lastBuild, lastTests };
}

export function realVoiceDeps(): VoiceDeps {
  return {
    activeProjectId: () => useAppStore.getState().activeProjectId,
    projectName: (id) => useAppStore.getState().projects.find((p) => p.id === id)?.name ?? null,
    projectCount: () => useAppStore.getState().projects.length,
    phrase: async (facts) => {
      if (useVoiceStore.getState().config?.naturalReplies === false) throw new Error("natural replies are off");
      const r = await getClient().call("voice.phrase", { facts, lang: currentLanguage(), timeoutMs: PHRASE_TIMEOUT_MS }, { timeoutMs: PHRASE_TIMEOUT_MS + 500 });
      return r.text;
    },
    listTasks: async (projectId) => (await getClient().call("tasks.list", { projectId })).tasks,
    listAgents: async (projectId) => (await getClient().call("agents.list", projectId ? { projectId } : {})).agents,
    pendingApprovals: () => useSecurityStore.getState().pending,
    resolveApproval: (id, decision) => resolveApproval(id, decision),
    stopProject: async (projectId) => (await getClient().call("emergency.stopProject", { projectId })).agentsStopped,
    stopAll: () => stopAllAi(),
    openPanel: (panel) => {
      if (panel === "settings") useSettingsStore.getState().openSettings();
      else workspaceActions.openPanel(panel);
    },
    switchProject: (projectId) => openProject(projectId),
    setDnd: (on) => useSettingsStore.getState().update({ doNotDisturb: on }),
    askManager: async (projectId, text) => {
      const r = await getClient().call("manager.chat", { projectId, text });
      return { replyMessageId: r.replyMessageId };
    },
    buildErrors,
    quality,
    // QA through the Phase 2 quality runner (the project's enabled checks).
    runQa: async (projectId) => {
      await startRun(projectId);
      workspaceActions.openPanel("tasks");
    },
  };
}
