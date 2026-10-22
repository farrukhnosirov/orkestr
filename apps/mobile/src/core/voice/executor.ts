/**
 * Mobile voice: recording → `voice.transcribe` (core cloud STT) → shared
 * intent parser (`@orkestr/client` voice) → RPC. Anything unrecognised goes
 * to the AI Manager. State-changing intents need `confirm` (except the
 * emergency stop, which runs at once — §52).
 */
import { type Intent, type IntentContext, needsConfirmation, parseIntent } from "@orkestr/client";

import type { Recording } from "../adapters";
import { type StepUpProvider, resolveWithStepUp } from "../approvals";
import type { RemoteClient } from "../connection";

export type VoiceOutcome =
  | { status: "ok"; intent: Intent; reply: string; data?: unknown }
  | { status: "needsConfirmation"; intent: Intent }
  | { status: "cancelled"; intent: Intent }
  | { status: "navigate"; intent: Intent; screen: string }
  | { status: "unsupported"; intent: Intent }
  | { status: "failed"; intent: Intent; error: string };

export interface ExecuteOptions {
  /** Asked for stop/approve/reject; absent → `needsConfirmation` is returned and nothing runs. */
  confirm?: (intent: Intent) => Promise<boolean>;
  /** Step-up for voice approve/reject of dangerous approvals. */
  stepUp?: StepUpProvider;
}

/** Context for the parser from the core (project + agent names). */
export async function loadIntentContext(client: RemoteClient, activeProjectId: string | null): Promise<IntentContext> {
  const [p, a] = await Promise.all([
    client.call("projects.list", {}).catch(() => ({ projects: [] })),
    client.call("agents.list", { activeOnly: true }).catch(() => ({ agents: [] })),
  ]);
  return {
    projects: p.projects.map((x) => ({ id: x.id, name: x.name })),
    agents: a.agents.map((x) => ({ id: x.id, name: x.name, projectId: x.projectId })),
    activeProjectId,
  };
}

const errCode = (e: unknown) => {
  const c = (e as { code?: unknown } | null)?.code;
  return typeof c === "string" ? c : e instanceof Error ? e.message : "internal";
};

export async function executeIntent(client: RemoteClient, intent: Intent, ctx: IntentContext, o: ExecuteOptions = {}): Promise<VoiceOutcome> {
  if (needsConfirmation(intent)) {
    if (!o.confirm) return { status: "needsConfirmation", intent };
    if (!(await o.confirm(intent))) return { status: "cancelled", intent };
  }
  try {
    switch (intent.kind) {
      case "empty":
        return { status: "unsupported", intent };
      case "stopAll":
      case "emergencyStop": {
        const r = await client.call("emergency.stopAll", {});
        return { status: "ok", intent, reply: `Stopped ${r.agentsStopped} agents.`, data: r };
      }
      case "stopProject": {
        if (!intent.project) return { status: "failed", intent, error: "project_not_found" };
        const r = await client.call("emergency.stopProject", { projectId: intent.project.id });
        return { status: "ok", intent, reply: `Stopped ${r.agentsStopped} agents in ${intent.project.name}.`, data: r };
      }
      case "approve":
      case "reject": {
        let id = intent.approvalId;
        if (!id) {
          const pending = (await client.call("approvals.list", { status: "Pending", limit: 1 })).approvals;
          id = pending[0]?.id;
        }
        if (!id) return { status: "failed", intent, error: "no_pending_approval" };
        const params = { approvalId: id, decision: intent.kind === "approve" ? ("Approve" as const) : ("Reject" as const) };
        const a = o.stepUp ? await resolveWithStepUp(client, params, o.stepUp) : (await client.call("approvals.resolve", params)).approval;
        return { status: "ok", intent, reply: a.status, data: a };
      }
      case "projectStatus": {
        const pid = intent.project?.id ?? ctx.activeProjectId;
        const snap = await client.call("dashboard.snapshot", {});
        const p = snap.projects.find((x) => x.projectId === pid);
        if (!p) return { status: "failed", intent, error: "project_not_found" };
        return {
          status: "ok",
          intent,
          reply: `${p.name}: ${p.activeAgents} agents active, ${p.runningTasks} tasks running, ${p.pendingApprovals} approvals pending.`,
          data: p,
        };
      }
      case "agentStatus": {
        if (!intent.agent) return { status: "failed", intent, error: "agent_not_found" };
        const r = await client.call("agents.get", { agentId: intent.agent.id });
        return { status: "ok", intent, reply: `${r.agent.name}: ${r.agent.state}${r.agent.activity ? ` — ${r.agent.activity}` : ""}`, data: r.agent };
      }
      case "openPanel":
        return { status: "navigate", intent, screen: intent.panel };
      case "showBuildErrors":
      case "switchProject":
      case "runQa":
      case "dnd":
        return { status: "unsupported", intent };
      case "askManager": {
        const pid = ctx.activeProjectId ?? ctx.projects[0]?.id;
        if (!pid) return { status: "failed", intent, error: "project_not_found" };
        const r = await client.call("manager.chat", { projectId: pid, text: intent.text });
        return { status: "ok", intent, reply: "", data: r };
      }
    }
  } catch (e) {
    return { status: "failed", intent, error: errCode(e) };
  }
}

/** Text (typed or transcribed) → intent → RPC. */
export function executeTranscript(client: RemoteClient, text: string, ctx: IntentContext, o: ExecuteOptions = {}): Promise<VoiceOutcome> {
  return executeIntent(client, parseIntent(text, ctx), ctx, o);
}

/**
 * Sends a recording to `voice.transcribe`. Errors (no consent, no STT
 * provider configured) propagate with the core's code — never a fake text.
 */
export async function transcribe(client: RemoteClient, rec: Recording, language?: string): Promise<string> {
  const r = await client.call("voice.transcribe", {
    audioBase64: rec.base64,
    mimeType: rec.mimeType,
    ...(language ? { language } : {}),
  });
  return r.text;
}
