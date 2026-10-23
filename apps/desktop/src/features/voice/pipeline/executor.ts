/**
 * Executes parsed voice intents (§33) through the same RPC methods and UI
 * actions as every other client (command registry, workspace, approvals).
 * All side effects go through `VoiceDeps`, so the routing and the replies
 * are unit-tested with fakes. Replies are short, localized sentences that
 * the HUD shows and TTS speaks.
 */
import type { AgentInstance, AgentState, Approval, Task } from "@orkestr/protocol";

import { t } from "../../../i18n";
import { agentActivity, agentName, approvalSummary } from "../../../lib/coreText";
import type { Intent, PanelTarget, ProjectRef } from "../intent/parser";
import { factsForModel, humanProjectName, naturalStatus, offersErrors, phraseNaturally } from "./natural";
import { EMPTY_QUALITY, projectFacts, type QualitySnapshot, summaryParts, type SummaryPart } from "./summary";

export interface BuildError {
  at: string;
  summary: string;
  exitCode: number | null;
}

export interface VoiceDeps {
  activeProjectId: () => string | null;
  projectName: (id: string) => string | null;
  listTasks: (projectId: string) => Promise<Task[]>;
  listAgents: (projectId: string | null) => Promise<AgentInstance[]>;
  /** Pending approvals (live, from the security store). */
  pendingApprovals: () => readonly Approval[];
  resolveApproval: (id: string, decision: "Approve" | "Reject") => Promise<void>;
  stopProject: (projectId: string) => Promise<number>;
  /** Emergency stop of all AI work (`emergency.stopAll`). */
  stopAll: () => Promise<boolean>;
  openPanel: (panel: PanelTarget) => void;
  switchProject: (projectId: string) => Promise<void>;
  setDnd: (on: boolean) => Promise<boolean>;
  askManager: (projectId: string, text: string) => Promise<{ replyMessageId: string }>;
  /** Recent build failures of a project (quality runner, then the event log). */
  buildErrors: (projectId: string) => Promise<BuildError[]>;
  /** Quality snapshot: quality runner builds / tests + open reviews (Phase 2). */
  quality: (projectId: string) => Promise<QualitySnapshot>;
  /**
   * Starts a QA run through the quality runner (`quality.run`). When absent,
   * the request goes to the Manager, which plans and runs the QA work.
   */
  runQa?: (projectId: string) => Promise<void>;
  /** Number of open projects (a status reply names the project only when there are several). */
  projectCount?: () => number;
  /**
   * Natural AI phrasing of reply facts (`voice.phrase`), when enabled and a
   * model is available; rejects otherwise (the templates are used).
   */
  phrase?: (facts: string) => Promise<string>;
}

export type CommandOutcome = "ok" | "failed" | "needsConfirmation" | "sentToManager" | "cancelled" | "notUnderstood";
export type ReplyTone = "ok" | "info" | "warn" | "error";

export interface CommandResult {
  outcome: CommandOutcome;
  reply: string;
  tone: ReplyTone;
  projectId: string | null;
  /** Manager reply to wait for (spoken when it arrives). */
  managerReply?: { projectId: string; messageId: string };
  /** Offered next step: a spoken "yes" runs it (e.g. show the build errors). */
  followUp?: Intent;
}

const result = (outcome: CommandOutcome, reply: string, tone: ReplyTone, projectId: string | null = null): CommandResult => ({
  outcome,
  reply,
  tone,
  projectId,
});

function agentStateText(state: AgentState): string {
  return t(`common:status.agent.${state}`);
}

function missingProject(intent: { query?: string }): CommandResult {
  return intent.query
    ? result("failed", t("voice:reply.projectNotFound", { name: intent.query }), "warn")
    : result("failed", t("voice:reply.noProject"), "warn");
}

/** Localized title of a panel target (panel registry titles; Settings has its own). */
export function panelTitle(panel: PanelTarget): string {
  return panel === "settings" ? t("shell:settings.title") : t(`shell:panels.${panel}.title`);
}

/** One summary sentence. */
export function summarySentence(p: SummaryPart): string {
  switch (p.id) {
    case "idle":
      return t("voice:summary.idle");
    case "noTasks":
      return t("voice:summary.noTasks");
    case "running":
      return t("voice:summary.running", { count: p.count });
    case "agents":
      return t("voice:summary.agents", { count: p.count });
    case "focus":
      return t("voice:summary.focus", { agent: agentName(p.agent), activity: agentActivity(p.activity) });
    case "completedToday":
      return t("voice:summary.completedToday", { count: p.count });
    case "failed":
      return t("voice:summary.failed", { count: p.count });
    case "blocked":
      return t("voice:summary.blocked", { count: p.count });
    case "approvals":
      return t("voice:summary.approvals", { count: p.count });
    case "buildFailed":
      return t("voice:summary.buildFailed");
    case "buildOk":
      return t("voice:summary.buildOk");
    case "testsFailed":
      return t("voice:summary.testsFailed", { count: p.count });
    case "openReviews":
      return t("voice:summary.openReviews", { count: p.count });
  }
}

/** The approval a spoken "approve"/"reject" refers to: newest pending, preferring the active project. */
export function targetApproval(pending: readonly Approval[], activeProjectId: string | null): Approval | null {
  const live = pending.filter((a) => a.status === "Pending");
  const sorted = [...live].sort((a, b) => b.requestedAt.localeCompare(a.requestedAt));
  return sorted.find((a) => a.projectId === activeProjectId) ?? sorted[0] ?? null;
}

/** The approval an approve/reject intent resolves: the bound one (exact id, still pending), else the current target. */
function approvalFor(intent: Extract<Intent, { kind: "approve" | "reject" }>, deps: Pick<VoiceDeps, "pendingApprovals" | "activeProjectId">): Approval | null {
  if (intent.approvalId !== undefined) {
    return deps.pendingApprovals().find((a) => a.id === intent.approvalId && a.status === "Pending") ?? null;
  }
  return targetApproval(deps.pendingApprovals(), deps.activeProjectId());
}

/**
 * Binds an approve/reject intent to the approval the confirmation reads out,
 * so a later "yes" resolves exactly that request even when a newer one
 * arrives meanwhile. Other intents are returned unchanged.
 */
export function bindApproval(intent: Intent, deps: Pick<VoiceDeps, "pendingApprovals" | "activeProjectId">): Intent {
  if ((intent.kind !== "approve" && intent.kind !== "reject") || intent.approvalId !== undefined) return intent;
  const a = targetApproval(deps.pendingApprovals(), deps.activeProjectId());
  return a ? { ...intent, approvalId: a.id } : intent;
}

/** The question asked before running an intent that needs confirmation. */
export function confirmationPrompt(intent: Intent, deps: Pick<VoiceDeps, "pendingApprovals" | "activeProjectId">): string {
  switch (intent.kind) {
    case "stopProject":
      return t("voice:confirm.stopProject", { name: intent.project?.name ?? "" });
    case "stopAll":
      return t("voice:confirm.stopAll");
    case "approve":
    case "reject": {
      const a = approvalFor(intent, deps);
      const key = intent.kind === "approve" ? "voice:confirm.approve" : "voice:confirm.reject";
      return t(key, { action: approvalSummary(a?.actionSummary ?? "") });
    }
    default:
      return "";
  }
}

/**
 * Pre-flight checks for intents that need confirmation: returns a failure
 * when there is nothing to confirm (unknown project, no pending approval).
 */
export function precheck(intent: Intent, deps: Pick<VoiceDeps, "pendingApprovals" | "activeProjectId">): CommandResult | null {
  if (intent.kind === "stopProject" && !intent.project) return missingProject(intent);
  if (intent.kind === "approve" || intent.kind === "reject") {
    const a = approvalFor(intent, deps);
    if (!a) return intent.approvalId !== undefined ? result("failed", t("voice:reply.approvalGone"), "warn") : result("failed", t("voice:reply.noApprovals"), "info");
    // Dangerous actions (§23, §70) are approved only in the app, never by a spoken "yes".
    if (intent.kind === "approve" && a.danger) return result("failed", t("voice:reply.dangerousInApp", { action: approvalSummary(a.actionSummary) }), "warn", a.projectId);
  }
  return null;
}

async function projectStatus(project: ProjectRef, deps: VoiceDeps): Promise<CommandResult> {
  const [tasks, agents, quality] = await Promise.all([
    deps.listTasks(project.id),
    deps.listAgents(project.id),
    deps.quality(project.id).catch(() => EMPTY_QUALITY),
  ]);
  const approvals = deps.pendingApprovals().filter((a) => a.status === "Pending" && a.projectId === project.id).length;
  const facts = projectFacts(tasks, agents, approvals, new Date(), quality);
  const parts = summaryParts(facts);
  const urgent = parts.some((p) => p.id === "failed" || p.id === "buildFailed" || p.id === "testsFailed" || p.id === "approvals");
  // Name the project only when the user asked about it by name; otherwise the
  // active project is implied and saying its slug sounds robotic.
  const mention = project.via === "spoken" ? humanProjectName(project.name) : null;
  const template = naturalStatus(facts, { project: mention });
  const reply = await phraseNaturally(deps.phrase, factsForModel(facts, mention), template);
  const r = result("ok", reply, urgent ? "warn" : "info", project.id);
  return offersErrors(facts) ? { ...r, followUp: { kind: "showBuildErrors", project } } : r;
}

/** Runs an intent (after confirmation, when one was needed). */
export async function executeIntent(intent: Intent, deps: VoiceDeps): Promise<CommandResult> {
  switch (intent.kind) {
    case "empty":
      return result("cancelled", t("voice:reply.didNotHear"), "info");

    case "emergencyStop":
    case "stopAll": {
      const ok = await deps.stopAll();
      return ok ? result("ok", t("voice:reply.stoppedAll"), "warn") : result("failed", t("voice:reply.stopFailed"), "error");
    }

    case "stopProject": {
      if (!intent.project) return missingProject(intent);
      const n = await deps.stopProject(intent.project.id);
      return result("ok", t("voice:reply.stoppedProject", { name: intent.project.name, count: n }), "warn", intent.project.id);
    }

    case "runQa": {
      if (!intent.project) return missingProject(intent);
      const p = intent.project;
      if (deps.runQa) {
        await deps.runQa(p.id);
        return result("ok", t("voice:reply.qaStarted", { name: p.name }), "ok", p.id);
      }
      const r = await deps.askManager(p.id, t("voice:managerPrompts.runQa", { name: p.name }));
      deps.openPanel("manager");
      return {
        ...result("sentToManager", t("voice:reply.qaSentToManager", { name: p.name }), "ok", p.id),
        managerReply: { projectId: p.id, messageId: r.replyMessageId },
      };
    }

    case "agentStatus": {
      if (!intent.agent) return result("failed", t("voice:reply.agentNotFound", { name: intent.query }), "warn");
      const agents = await deps.listAgents(intent.agent.projectId);
      const a = agents.find((x) => x.id === intent.agent?.id);
      if (!a) return result("failed", t("voice:reply.agentNotFound", { name: intent.agent.name }), "warn");
      const state = agentStateText(a.state);
      const reply = a.activity
        ? t("voice:reply.agentDoing", { name: agentName(a.name), state, activity: agentActivity(a.activity) })
        : t("voice:reply.agentState", { name: agentName(a.name), state });
      return result("ok", reply, a.state === "Error" ? "error" : "info", a.projectId);
    }

    case "projectStatus":
      if (!intent.project) return missingProject(intent);
      return projectStatus(intent.project, deps);

    case "showBuildErrors": {
      if (!intent.project) return missingProject({});
      const errors = await deps.buildErrors(intent.project.id);
      deps.openPanel("logs");
      const last = errors[0];
      if (!last) return result("ok", t("voice:reply.noBuildErrors", { name: intent.project.name }), "ok", intent.project.id);
      return result(
        "ok",
        t("voice:reply.buildErrors", { count: errors.length, name: intent.project.name, summary: last.summary || t("voice:reply.noSummary") }),
        "error",
        intent.project.id,
      );
    }

    case "openPanel":
      deps.openPanel(intent.panel);
      return result("ok", t("voice:reply.opened", { panel: panelTitle(intent.panel) }), "ok");

    case "switchProject":
      if (!intent.project) return missingProject(intent);
      await deps.switchProject(intent.project.id);
      return result("ok", t("voice:reply.switched", { name: intent.project.name }), "ok", intent.project.id);

    case "approve":
    case "reject": {
      const blocked = precheck(intent, deps);
      if (blocked) return blocked;
      const a = approvalFor(intent, deps);
      if (!a) return result("failed", t("voice:reply.noApprovals"), "info");
      await deps.resolveApproval(a.id, intent.kind === "approve" ? "Approve" : "Reject");
      return result(
        "ok",
        intent.kind === "approve" ? t("voice:reply.approved", { action: approvalSummary(a.actionSummary) }) : t("voice:reply.rejected", { action: approvalSummary(a.actionSummary) }),
        "ok",
        a.projectId,
      );
    }

    case "dnd": {
      const ok = await deps.setDnd(intent.on);
      if (!ok) return result("failed", t("voice:reply.dndFailed"), "error");
      return result("ok", intent.on ? t("voice:reply.dndOn") : t("voice:reply.dndOff"), "ok");
    }

    case "askManager": {
      const pid = deps.activeProjectId();
      if (!pid) return result("notUnderstood", t("voice:reply.askNeedsProject"), "warn");
      const r = await deps.askManager(pid, intent.text);
      deps.openPanel("manager");
      return {
        ...result("sentToManager", t("voice:reply.sentToManager"), "info", pid),
        managerReply: { projectId: pid, messageId: r.replyMessageId },
      };
    }
  }
}
