/**
 * Read-side loaders for the Home / Review / Editor screens. Each section is
 * loaded independently: a failing method (e.g. role without access) shows
 * an error for that section, never fake data.
 */
import type {
  AgentInstance,
  AgentTranscriptResult,
  Approval,
  DashboardSnapshot,
  DiffResult,
  EmergencyStatusResult,
  EmergencyStopResult,
  Node,
  NodeResourcesResult,
  ReadFileResult,
  RemoteStatus,
  Task,
} from "@orkestr/protocol";

import type { StepUpProvider } from "./approvals";
import { callWithStepUp } from "./approvals";
import type { RemoteClient } from "./connection";

export type Section<T> = { ok: true; value: T } | { ok: false; error: string };

export interface MonitoringData {
  dashboard: Section<DashboardSnapshot>;
  nodes: Section<Node[]>;
  resources: Section<NodeResourcesResult>;
  agents: Section<AgentInstance[]>;
  approvals: Section<Approval[]>;
  emergency: Section<EmergencyStatusResult>;
}

const code = (e: unknown) => {
  const c = (e as { code?: unknown } | null)?.code;
  return typeof c === "string" ? c : "internal";
};

async function section<T>(p: Promise<T>): Promise<Section<T>> {
  try {
    return { ok: true, value: await p };
  } catch (e) {
    return { ok: false, error: code(e) };
  }
}

export async function loadMonitoring(client: RemoteClient): Promise<MonitoringData> {
  const [dashboard, nodes, resources, agents, approvals, emergency] = await Promise.all([
    section(client.call("dashboard.snapshot", {})),
    section(client.call("nodes.list", {}).then((r) => r.nodes)),
    section(client.call("nodes.resources", {})),
    section(client.call("agents.list", { activeOnly: true }).then((r) => r.agents)),
    section(client.call("approvals.list", { status: "Pending" }).then((r) => r.approvals)),
    section(client.call("emergency.status", {})),
  ]);
  return { dashboard, nodes, resources, agents, approvals, emergency };
}

// ------------------------------------------------------------- emergency

export type EmergencyScope = { kind: "all" } | { kind: "project"; projectId: string } | { kind: "agent"; agentId: string };

export async function emergencyStop(client: RemoteClient, scope: EmergencyScope): Promise<EmergencyStopResult | null> {
  switch (scope.kind) {
    case "all":
      return client.call("emergency.stopAll", {});
    case "project":
      return client.call("emergency.stopProject", { projectId: scope.projectId });
    case "agent":
      await client.call("agents.stop", { agentId: scope.agentId });
      return null;
  }
}

/** Remote kill switch: the core drops the tunnel and does not redial. */
export function disableRemote(client: RemoteClient, stepUp?: StepUpProvider): Promise<RemoteStatus> {
  return stepUp ? callWithStepUp(client, "remote.disable", {}, stepUp) : client.call("remote.disable", {});
}

// ---------------------------------------------------------------- review

export async function loadTasks(client: RemoteClient, projectId: string): Promise<Task[]> {
  return (await client.call("tasks.list", { projectId })).tasks;
}

export function loadDiff(client: RemoteClient, projectId: string, worktree?: string): Promise<DiffResult> {
  return client.call("vcs.diff", { projectId, target: { kind: "Head" }, includeUntracked: true, ...(worktree ? { worktree } : {}) });
}

export function loadTranscript(client: RemoteClient, agentId: string, limit = 100): Promise<AgentTranscriptResult> {
  return client.call("agents.transcript", { agentId, limit });
}

// ---------------------------------------------------------------- editor

export type SaveResult = { status: "saved"; hash: string } | { status: "conflict"; actualHash?: string };

export function readFile(client: RemoteClient, projectId: string, path: string): Promise<ReadFileResult> {
  return client.call("files.read", { projectId, path, maxBytes: 1024 * 1024 });
}

/** Writes with the hash guard: a concurrent change answers `conflict` instead of overwriting. */
export async function saveFile(client: RemoteClient, projectId: string, path: string, content: string, expectedHash: string): Promise<SaveResult> {
  try {
    const r = await client.call("files.write", { projectId, path, content, expectedHash });
    return { status: "saved", hash: r.hash };
  } catch (e) {
    if (code(e) !== "conflict") throw e;
    const d = (e as { data?: { actualHash?: unknown } }).data;
    return { status: "conflict", ...(typeof d?.actualHash === "string" ? { actualHash: d.actualHash } : {}) };
  }
}
