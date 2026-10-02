import type { Event, EventKindName } from "@orkestr/protocol";

export type EventCategory = "project" | "task" | "agent" | "tool" | "build" | "approval" | "provider" | "system" | "log";
export type EventSeverity = "info" | "success" | "warning" | "error";

const CATEGORY: Partial<Record<EventKindName, EventCategory>> = {
  ProjectCreated: "project",
  ProjectUpdated: "project",
  ProjectDeleted: "project",
  TaskCreated: "task",
  TaskUpdated: "task",
  TaskStatusChanged: "task",
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
    case "BudgetThreshold":
    case "ApprovalRequested":
    case "RollbackPerformed":
      return "warning";
    case "BuildSucceeded":
    case "DeploymentFinished":
      return "success";
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

/** One-line human description of an event. */
export function describeEvent(event: Event): string {
  switch (event.kind) {
    case "ProjectCreated":
      return `Project “${event.payload.project.name}” created`;
    case "ProjectUpdated":
      return `Project “${event.payload.project.name}” updated`;
    case "ProjectDeleted":
      return `Project …${short(event.payload.projectId)} deleted`;
    case "TaskCreated":
      return `Task “${event.payload.task.title}” created`;
    case "TaskUpdated":
      return `Task “${event.payload.task.title}” updated`;
    case "TaskStatusChanged":
      return `Task …${short(event.payload.taskId)}: ${event.payload.from} → ${event.payload.to}`;
    case "AgentSpawned":
      return `Agent “${event.payload.agent.name}” spawned`;
    case "AgentStarted":
      return `Agent …${short(event.payload.agentId)} started`;
    case "AgentStateChanged":
      return `Agent …${short(event.payload.agentId)}: ${event.payload.from} → ${event.payload.to}${event.payload.activity ? ` (${event.payload.activity})` : ""}`;
    case "AgentStopped":
      return `Agent …${short(event.payload.agentId)} stopped (${event.payload.reason})`;
    case "AgentMessage":
    case "ManagerMessage":
      return event.payload.delta.trim() || "(message)";
    case "ToolCallStarted":
      return `Tool ${event.payload.toolCall.tool} started`;
    case "ToolCallFinished":
      return `Tool call …${short(event.payload.toolCallId)} ${event.payload.status} in ${event.payload.durationMs} ms`;
    case "FileChanged":
      return `${event.payload.change}: ${event.payload.path}`;
    case "BuildStarted":
      return `Build started: ${event.payload.command}`;
    case "BuildFailed":
      return `Build failed: ${event.payload.summary}`;
    case "BuildSucceeded":
      return `Build succeeded in ${event.payload.durationMs} ms`;
    case "TestRunFinished":
      return `Tests: ${event.payload.passed} passed, ${event.payload.failed} failed, ${event.payload.skipped} skipped`;
    case "ReviewFinished":
      return `Review ${event.payload.verdict}: ${event.payload.summary}`;
    case "ApprovalRequested":
      return `Approval requested: ${event.payload.approval.actionSummary}`;
    case "ApprovalResolved":
      return `Approval …${short(event.payload.approvalId)} ${event.payload.status}`;
    case "ProviderStatusChanged":
      return `Provider …${short(event.payload.providerId)} ${event.payload.health}`;
    case "ProviderRateLimited":
      return `Provider …${short(event.payload.providerId)} rate limited${event.payload.retryAfterSecs != null ? ` (retry in ${event.payload.retryAfterSecs}s)` : ""}`;
    case "ProviderFallback":
      return `Fallback ${event.payload.from.model} → ${event.payload.to.model}: ${event.payload.reason}`;
    case "BudgetThreshold":
      return `Budget ${event.payload.scope}: $${event.payload.spentUsd.toFixed(2)} of $${event.payload.limitUsd.toFixed(2)}`;
    case "SnapshotCreated":
      return `Snapshot “${event.payload.snapshot.label}” created`;
    case "RollbackPerformed":
      return `Rollback (${event.payload.scope}): ${event.payload.filesRestored} files restored`;
    case "TerminalOutput":
      return event.payload.data.replace(/\s+/g, " ").trim().slice(0, 200) || "(output)";
    case "TerminalExited":
      return `Terminal …${short(event.payload.terminalId)} exited (${event.payload.exitCode ?? "?"})`;
    case "NodeStatus":
      return `Node ${event.payload.node.name}: ${event.payload.node.status}`;
    case "NodeOffline":
      return `Node …${short(event.payload.nodeId)} offline`;
    case "SyncCompleted":
      return `Sync (${event.payload.backend}): ${event.payload.items} items`;
    case "DeploymentFinished":
      return `Deployment ${event.payload.deployment.status}`;
    case "MemoryUpdated":
      return `Memory “${event.payload.item.title}” ${event.payload.deleted ? "deleted" : "updated"}`;
    case "DecisionRecorded":
      return `Decision: ${event.payload.decision.title}`;
    case "EmergencyStop":
      return `EMERGENCY STOP (${event.payload.scope.scope}): ${event.payload.agentsStopped} agents stopped`;
    case "Notification":
      return `${event.payload.notification.title}: ${event.payload.notification.body}`;
    case "Log":
      return event.payload.message || event.payload.target;
    case "SettingsChanged":
      return `Settings changed: ${event.payload.changed.join(", ")}`;
  }
}

/** `HH:MM:SS.mmm` in local time. */
export function formatTime(ts: string): string {
  const d = new Date(ts);
  if (Number.isNaN(d.getTime())) return ts;
  const pad = (n: number, w = 2) => String(n).padStart(w, "0");
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}.${pad(d.getMilliseconds(), 3)}`;
}
