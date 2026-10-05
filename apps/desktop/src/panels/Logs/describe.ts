import type { Event, EventKindName } from "@orkestr/protocol";

import { formatCurrency, formatDuration, t } from "../../i18n";
import { agentActivity, agentName, snapshotLabel } from "../../lib/coreText";

export type EventCategory = "project" | "task" | "agent" | "tool" | "build" | "approval" | "provider" | "system" | "log";
export type EventSeverity = "info" | "success" | "warning" | "error";

const CATEGORY: Partial<Record<EventKindName, EventCategory>> = {
  ProjectCreated: "project",
  ProjectUpdated: "project",
  ProjectDeleted: "project",
  TaskCreated: "task",
  TaskUpdated: "task",
  TaskStatusChanged: "task",
  TaskDeleted: "task",
  AgentSpawned: "agent",
  AgentStarted: "agent",
  AgentStateChanged: "agent",
  AgentStopped: "agent",
  AgentMessage: "agent",
  ManagerMessage: "agent",
  ToolCallStarted: "tool",
  ToolCallFinished: "tool",
  FileChanged: "tool",
  TerminalOutput: "tool",
  TerminalExited: "tool",
  BuildStarted: "build",
  BuildFailed: "build",
  BuildSucceeded: "build",
  TestRunFinished: "build",
  ReviewFinished: "build",
  PipelineUpdated: "build",
  PipelineStageChanged: "build",
  HealingAttemptStarted: "build",
  ApprovalRequested: "approval",
  ApprovalResolved: "approval",
  ProviderStatusChanged: "provider",
  ProviderRateLimited: "provider",
  ProviderFallback: "provider",
  BudgetThreshold: "provider",
  Log: "log",
};

export function categoryOf(event: Event): EventCategory {
  return CATEGORY[event.kind] ?? "system";
}

export function severityOf(event: Event): EventSeverity {
  switch (event.kind) {
    case "Log":
      return event.payload.level === "error" ? "error" : event.payload.level === "warn" ? "warning" : "info";
    case "BuildFailed":
    case "NodeOffline":
    case "EmergencyStop":
      return "error";
    case "ProviderRateLimited":
    case "ProviderFallback":
    case "MergeRiskDetected":
    case "HealingAttemptStarted":
    case "BudgetThreshold":
    case "ApprovalRequested":
    case "RollbackPerformed":
      return "warning";
    case "BuildSucceeded":
    case "DeploymentFinished":
      return "success";
    case "PipelineUpdated":
      return event.payload.pipeline.status === "Passed"
        ? "success"
        : event.payload.pipeline.status === "Failed"
          ? "error"
          : event.payload.pipeline.status === "Escalated" || event.payload.pipeline.status === "AwaitingUser"
            ? "warning"
            : "info";
    case "PipelineStageChanged":
      return event.payload.status === "Failed" ? "warning" : event.payload.status === "Passed" ? "success" : "info";
    case "TestRunFinished":
      return event.payload.failed > 0 ? "error" : "success";
    case "ProjectCreated":
      return "success";
    case "ProjectDeleted":
      return "warning";
    case "Notification":
      return event.payload.notification.level === "Error" || event.payload.notification.level === "Critical"
        ? "error"
        : event.payload.notification.level === "Warning"
          ? "warning"
          : event.payload.notification.level === "Success"
            ? "success"
            : "info";
    default:
      return "info";
  }
}

const short = (id: string | null | undefined) => (id ? id.slice(-6) : "—");

/** Localized task status (falls back to the raw value). */
function taskStatus(status: string): string {
  return t(`common:status.task.${status}` as "common:status.task.Running", { defaultValue: status });
}

/** Localized agent state (falls back to the raw value). */
function agentState(state: string): string {
  return t(`common:status.agent.${state}` as "common:status.agent.Idle", { defaultValue: state });
}

/** Localized label of a budget scope string sent by the core ("project", "task", …; falls back to the raw value). */
function budgetScope(scope: string): string {
  return t(`logs:enums.budgetScope.${scope.toLowerCase()}` as "logs:enums.budgetScope.project", { defaultValue: scope });
}

const FILE_CHANGE_KEY = {
  Created: "events:fileCreated",
  Modified: "events:fileModified",
  Deleted: "events:fileDeleted",
  Renamed: "events:fileRenamed",
} as const;

/** One-line human description of an event, in the current UI language. */
export function describeEvent(event: Event): string {
  switch (event.kind) {
    case "ProjectCreated":
      return t("events:projectCreated", { name: event.payload.project.name });
    case "ProjectUpdated":
      return t("events:projectUpdated", { name: event.payload.project.name });
    case "ProjectDeleted":
      return t("events:projectDeleted", { id: short(event.payload.projectId) });
    case "TaskCreated":
      return t("events:taskCreated", { title: event.payload.task.title });
    case "TaskUpdated":
      return t("events:taskUpdated", { title: event.payload.task.title });
    case "TaskStatusChanged":
      return t("events:taskStatusChanged", {
        id: short(event.payload.taskId),
        from: taskStatus(event.payload.from),
        to: taskStatus(event.payload.to),
      });
    case "TaskDeleted":
      return t("events:taskDeleted", { id: short(event.payload.taskId) });
    case "AgentSpawned":
      return t("events:agentSpawned", { name: agentName(event.payload.agent.name) });
    case "AgentStarted":
      return t("events:agentStarted", { id: short(event.payload.agentId) });
    case "AgentStateChanged": {
      const vars = { id: short(event.payload.agentId), from: agentState(event.payload.from), to: agentState(event.payload.to) };
      return event.payload.activity
        ? t("events:agentStateChangedActivity", { ...vars, activity: agentActivity(event.payload.activity) })
        : t("events:agentStateChanged", vars);
    }
    case "AgentStopped":
      return t("events:agentStopped", { id: short(event.payload.agentId), reason: t(`logs:enums.stopReason.${event.payload.reason}`) });
    case "AgentMessage":
    case "ManagerMessage":
      return event.payload.delta.trim() || t("events:message");
    case "ToolCallStarted":
      return t("events:toolStarted", { tool: event.payload.toolCall.tool });
    case "ToolCallFinished":
      return t("events:toolFinished", {
        id: short(event.payload.toolCallId),
        status: t(`logs:enums.toolStatus.${event.payload.status}`),
        duration: formatDuration(event.payload.durationMs),
      });
    case "FileChanged":
      return t(FILE_CHANGE_KEY[event.payload.change], { path: event.payload.path, oldPath: event.payload.oldPath ?? "?" });
    case "BuildStarted":
      return t("events:buildStarted", { command: event.payload.command });
    case "BuildFailed":
      return t("events:buildFailed", { summary: event.payload.summary });
    case "BuildSucceeded":
      return t("events:buildSucceeded", { duration: formatDuration(event.payload.durationMs) });
    case "TestRunFinished":
      return t("events:testsFinished", {
        passed: event.payload.passed,
        failed: event.payload.failed,
        skipped: event.payload.skipped,
      });
    case "ReviewFinished":
      return t("events:reviewFinished", { verdict: t(`logs:enums.reviewVerdict.${event.payload.verdict}`), summary: event.payload.summary });
    case "ApprovalRequested":
      return t("events:approvalRequested", { summary: event.payload.approval.actionSummary });
    case "ApprovalResolved":
      return t("events:approvalResolved", {
        id: short(event.payload.approvalId),
        status: t(`common:status.approval.${event.payload.status}`),
      });
    case "ProviderStatusChanged":
      return t("events:providerStatus", {
        id: short(event.payload.providerId),
        health: t(`common:status.health.${event.payload.health}`),
      });
    case "ProviderRateLimited":
      return event.payload.retryAfterSecs != null
        ? t("events:providerRateLimitedRetry", { id: short(event.payload.providerId), seconds: event.payload.retryAfterSecs })
        : t("events:providerRateLimited", { id: short(event.payload.providerId) });
    case "ProviderFallback":
      return t("events:providerFallback", {
        from: event.payload.from.model,
        to: event.payload.to.model,
        reason: event.payload.reason,
      });
    case "BudgetThreshold":
      return t("events:budgetThreshold", {
        scope: budgetScope(event.payload.scope),
        spent: formatCurrency(event.payload.spentUsd),
        limit: formatCurrency(event.payload.limitUsd),
      });
    case "SnapshotCreated":
      return t("events:snapshotCreated", { label: snapshotLabel(event.payload.snapshot.label) });
    case "RollbackPerformed":
      return t("events:rollbackPerformed", { scope: t(`logs:enums.rollbackScope.${event.payload.scope}`), count: event.payload.filesRestored });
    case "TerminalOutput":
      return event.payload.data.replace(/\s+/g, " ").trim().slice(0, 200) || t("events:output");
    case "TerminalExited":
      return t("events:terminalExited", { id: short(event.payload.terminalId), code: event.payload.exitCode ?? "?" });
    case "NodeStatus":
      return t("events:nodeStatus", { name: event.payload.node.name, status: t(`logs:enums.nodeStatus.${event.payload.node.status}`) });
    case "NodeOffline":
      return t("events:nodeOffline", { id: short(event.payload.nodeId) });
    case "SyncCompleted":
      return t("events:syncCompleted", { backend: event.payload.backend, count: event.payload.items });
    case "DeploymentFinished":
      return t("events:deploymentFinished", { status: t(`logs:enums.deploymentStatus.${event.payload.deployment.status}`) });
    case "MemoryUpdated":
      return event.payload.deleted
        ? t("events:memoryDeleted", { title: event.payload.item.title })
        : t("events:memoryUpdated", { title: event.payload.item.title });
    case "DecisionRecorded":
      return t("events:decisionRecorded", { title: event.payload.decision.title });
    case "EmergencyStop":
      return t("events:emergencyStop", { scope: t(`logs:enums.emergencyScope.${event.payload.scope.scope}`), count: event.payload.agentsStopped });
    case "Notification":
      return t("events:notification", { title: event.payload.notification.title, body: event.payload.notification.body });
    case "Log":
      return event.payload.message || event.payload.target;
    case "SettingsChanged":
      return t("events:settingsChanged", { keys: event.payload.changed.join(", ") });
    case "AgentUpdated":
      return t("events:agentUpdated", {
        name: agentName(event.payload.agent.name),
        state: agentState(event.payload.agent.state),
        count: event.payload.agent.turnsUsed,
      });
    case "ManagerMessageAdded": {
      const role = event.payload.message.role;
      const who = role === "User" ? "user" : role === "Manager" ? "manager" : "event";
      return t("events:managerMessage", {
        who: t(`events:managerWho.${who}`),
        text: event.payload.message.text.slice(0, 120),
      });
    }
    case "ManagerPlanUpdated":
      return t("events:managerPlan", {
        goal: event.payload.plan.goal,
        status: t(`logs:enums.planStatus.${event.payload.plan.status}`),
        count: event.payload.plan.tasks.length,
      });
    case "ManagerRunUpdated": {
      const r = event.payload.run;
      const done = r.tasks.filter((x) => x.status === "Completed").length;
      return t("events:managerRun", { status: t(`logs:enums.runStatus.${r.status}`), done, total: r.tasks.length });
    }
    case "ReviewUpdated": {
      const r = event.payload.review;
      return t("events:reviewUpdated", {
        kind: t(`reviews:kind.${r.kind}`),
        status: r.verdict && r.status === "Finished" ? t(`reviews:verdict.${r.verdict}`) : t(`events:reviewStatus.${r.status}`),
        count: r.findings.length,
      });
    }
    case "ConsensusUpdated": {
      const c = event.payload.run;
      return t("events:consensusUpdated", { status: t(`consensus:status.${c.status}`), question: c.question.slice(0, 120) });
    }
    case "ArchitectureMapUpdated": {
      const a = event.payload;
      return t("events:architectureMapUpdated", {
        modules: a.stats.modules,
        services: a.stats.services,
        changes: a.addedNodes + a.removedNodes + a.changedNodes + a.changedEdges,
      });
    }
    case "MergeProposalUpdated": {
      const p = event.payload.proposal;
      return t("events:mergeProposal", { target: p.target, status: t(`merge:status.${p.status}`) });
    }
    case "SchedulerUpdated":
      return event.payload.settingsChanged
        ? t("events:schedulerSettings")
        : t("events:schedulerUpdated", { ...event.payload.counts });
    case "MergeRiskDetected": {
      const r = event.payload.risk;
      return t("events:mergeRisk", { count: Math.max(r.files.length, r.conflicts.length) });
    }
    // Phase 2: Agent Teams and the AI Product Manager.
    case "TeamUpdated":
      return t("events:teamUpdated", { name: event.payload.team.team.name });
    case "TeamDeleted":
      return t("events:teamDeleted", { id: short(event.payload.teamId) });
    case "TeamLeadRunUpdated":
      return t("events:teamLeadRun", {
        title: event.payload.lead.title,
        status: t(`teams:leadStatus.${event.payload.lead.status}`),
        count: event.payload.lead.subtasks.length,
      });
    case "PmDraftUpdated":
      return t("events:pmDraft", { title: event.payload.draft.epic.title, status: t(`product:status.${event.payload.draft.status}`) });
    // Phase 2: quality pipeline + self-healing.
    case "PipelineUpdated":
      return t("events:pipelineUpdated", {
        task: short(event.payload.pipeline.taskId),
        status: t(`pipeline:status.${event.payload.pipeline.status}`),
      });
    case "PipelineStageChanged":
      return t("events:pipelineStage", {
        stage: t(`pipeline:stage.${event.payload.stage}`),
        status: t(`pipeline:stageStatus.${event.payload.status}`),
      });
    case "HealingAttemptStarted":
      return t("events:healingAttempt", { n: event.payload.attempt, summary: event.payload.summary });
  }
}

/** `HH:MM:SS.mmm` in local time. */
export function formatTime(ts: string): string {
  const d = new Date(ts);
  if (Number.isNaN(d.getTime())) return ts;
  const pad = (n: number, w = 2) => String(n).padStart(w, "0");
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}.${pad(d.getMilliseconds(), 3)}`;
}
