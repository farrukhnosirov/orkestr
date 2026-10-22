/**
 * Nodes panel helpers (Phase 6, ADR-0033 / ADR-0039): status tones, resource
 * summaries, job ownership and the worker-side "node" pairing calls.
 */
import type {
  DevicePairingStart,
  DeviceInfo,
  JobState,
  JobView,
  ListNodesResult,
  Node,
  NodeSnapshot,
  NodeStatus,
  TrustLevel,
} from "@orkestr/protocol";
import type { Status, Tone } from "@orkestr/ui";

import { getClient } from "../../core/client";

export { isUnimplemented } from "../remoteSync/logic";

export const TRUST_LEVELS: readonly TrustLevel[] = ["restricted", "standard", "trusted"];

/** Badge tone of a node health state. */
export function statusTone(s: NodeStatus): Tone {
  switch (s) {
    case "Online":
      return "success";
    case "Busy":
      return "info";
    case "Suspect":
    case "Degraded":
    case "Draining":
      return "warning";
    case "Offline":
      return "neutral";
    case "Revoked":
      return "danger";
  }
}

/** Status dot of a node health state. */
export function statusDot(s: NodeStatus): Status {
  switch (s) {
    case "Online":
      return "online";
    case "Busy":
      return "busy";
    case "Suspect":
    case "Degraded":
    case "Draining":
      return "warning";
    case "Revoked":
      return "error";
    case "Offline":
      return "offline";
  }
}

/** Badge tone of a job state. */
export function jobTone(s: JobState): Tone {
  switch (s) {
    case "completed":
      return "success";
    case "running":
    case "offered":
      return "info";
    case "migrating":
    case "pending":
      return "warning";
    case "failed":
    case "infeasible":
      return "danger";
    case "cancelled":
      return "neutral";
  }
}

const TERMINAL: ReadonlySet<JobState> = new Set(["completed", "failed", "cancelled", "infeasible"]);
export const isTerminalJob = (j: JobView): boolean => TERMINAL.has(j.state);

/** Node the job runs on now (owner), else the placement target. */
export function jobNodeId(j: JobView): string | null {
  if (j.owner?.kind === "node") return j.owner.nodeId;
  if (j.placement?.decision === "place") return j.placement.nodeId;
  return null;
}

/** Wraps a pre-Phase-6 `Node` (no snapshot) as a read-only snapshot. */
function fromNode(n: Node): NodeSnapshot {
  return {
    node: n,
    status: n.status,
    trustLevel: "trusted",
    allowedProjects: ["*"],
    slots: { max: 0, used: 0 },
    leases: [],
    ...(n.lastHeartbeatAt ? { lastHeartbeatAt: n.lastHeartbeatAt } : {}),
    isCoordinator: n.isLocal,
    labels: {},
    modelDigests: [],
  };
}

/** Snapshots of `nodes.list` (coordinator first), falling back to plain nodes. */
export function snapshotsOf(r: ListNodesResult): NodeSnapshot[] {
  const list = r.snapshots?.length ? r.snapshots : r.nodes.map(fromNode);
  return [...list].sort((a, b) => Number(b.isCoordinator) - Number(a.isCoordinator));
}

/** Parses the "allowed projects" field (`*` or comma separated ids). */
export function parseProjects(text: string): string[] {
  const items = text
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
  return items.length ? items : ["*"];
}

/** Bytes → MiB for VRAM displays. */
export const toMb = (bytes: number): number => Math.round(bytes / (1024 * 1024));

// ── worker side: "Allow this computer to be a node" ───────────────────────
// `devices.startPairing` / `devices.approve` take a `kind: "node"` pairing with
// a trust level (WP-A). The generated param types predate those fields, so the
// extra fields are passed through an explicit widening here, in one place.

export type NodePairing = DevicePairingStart & { sas?: string; deviceName?: string };

type Caller = (method: string, params: unknown) => Promise<unknown>;
const rawCall = (): Caller => {
  const c = getClient() as unknown as { call: Caller };
  return (m, p) => c.call(m, p);
};

/** Starts a `kind: "node"` pairing on this (worker) computer. */
export async function startNodePairing(trustLevel: TrustLevel): Promise<NodePairing> {
  return (await rawCall()("devices.startPairing", { kind: "node", trustLevel })) as NodePairing;
}

/** Approves the coordinator after the SAS matched on both screens. */
export async function approveNodePairing(pairingId: string, trustLevel: TrustLevel): Promise<DeviceInfo> {
  return (await rawCall()("devices.approve", {
    pairingId,
    sasConfirmed: true,
    role: "operator",
    remoteTerminal: false,
    kind: "node",
    trustLevel,
  })) as DeviceInfo;
}
