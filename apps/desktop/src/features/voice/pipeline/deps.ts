/**
 * Real `VoiceDeps`: RPC through `@orkestr/client`, UI actions through the
 * workspace bridge, settings, the security store and the command registry.
 */
import { topic } from "@orkestr/protocol";

import { getClient, openProject } from "../../../core/client";
import { workspaceActions } from "../../../components/workspaceActions";
import { useAppStore } from "../../../store/app";
import { stopAllAi } from "../../commands/coreCommands";
import { resolveApproval } from "../../security/store";
import { useSecurityStore } from "../../security/store";
import { useSettingsStore } from "../../settings/settingsStore";
import type { BuildError, VoiceDeps } from "./executor";
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

async function qualityFromLog(projectId: string): Promise<QualitySnapshot> {
  // INTEGRATOR(P2): replace with the quality pipeline's latest build/test/review state.
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
    buildErrors: buildErrorsFromLog,
    quality: qualityFromLog,
    // INTEGRATOR(P2): runQa: (projectId) => getClient().call("quality.run", { projectId, … })
  };
}
