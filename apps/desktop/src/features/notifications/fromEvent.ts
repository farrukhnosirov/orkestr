/**
 * Maps core events to in-app notifications (§33 notification center).
 * `Notification` events are shown as-is; a few important event kinds are
 * turned into notifications so the user notices them even when the
 * relevant panel is closed. Pure and unit-tested.
 */
import type { Event, NotificationLevel } from "@orkestr/protocol";

export interface AppNotification {
  /** Stable id (event id, or `local-…` for UI-originated notices). */
  id: string;
  ts: string;
  level: NotificationLevel;
  title: string;
  body: string;
  projectId: string | null;
  /** Source event kind (`Notification`, `BuildFailed`, …) or `local`. */
  source: string;
  read: boolean;
}

function base(ev: Event, level: NotificationLevel, title: string, body: string): AppNotification {
  return { id: ev.id, ts: ev.ts, level, title, body, projectId: ev.projectId, source: ev.kind, read: false };
}

/** Returns the notification for an event, or `null` when the event is not notification-worthy. */
export function notificationFromEvent(ev: Event): AppNotification | null {
  switch (ev.kind) {
    case "Notification": {
      const n = ev.payload.notification;
      return { ...base(ev, n.level, n.title, n.body), id: n.id, ts: n.ts, projectId: n.projectId, read: n.read };
    }
    case "ApprovalRequested": {
      const a = ev.payload.approval;
      return base(ev, a.risk === "Critical" || a.risk === "High" ? "Warning" : "Info", "Approval needed", a.actionSummary);
    }
    case "BuildFailed":
      return base(ev, "Error", "Build failed", ev.payload.summary || `Exit code ${ev.payload.exitCode ?? "?"}`);
    case "TaskStatusChanged": {
      const { to, reason } = ev.payload;
      if (to === "Failed") return base(ev, "Error", "Task failed", reason ?? "A task failed.");
      if (to === "Blocked") return base(ev, "Warning", "Task blocked", reason ?? "A task is blocked.");
      if (to === "Completed") return base(ev, "Success", "Task completed", reason ?? "A task finished successfully.");
      return null;
    }
    case "BudgetThreshold": {
      const p = ev.payload;
      const pct = Math.round(p.fraction * 100);
      return base(
        ev,
        p.fraction >= 1 ? "Error" : "Warning",
        `Budget ${pct}% used`,
        `${p.scope}: $${p.spentUsd.toFixed(2)} of $${p.limitUsd.toFixed(2)}`,
      );
    }
    case "EmergencyStop":
      return base(ev, "Critical", "Emergency stop", `${ev.payload.agentsStopped} agent(s) stopped.`);
    case "ProviderRateLimited": {
      const p = ev.payload;
      const retry = p.retryAfterSecs != null ? ` Retrying in ${p.retryAfterSecs}s.` : "";
      return base(ev, "Warning", "Provider rate limited", `${p.model ?? "A model"} hit its rate limit.${retry}`);
    }
    case "RollbackPerformed":
      return base(ev, "Info", "Rollback performed", `${ev.payload.filesRestored} file(s) restored.`);
    case "NodeOffline":
      return base(ev, "Warning", "Node offline", "A node stopped sending heartbeats.");
    default:
      return null;
  }
}

/** Whether a notification should pop up as a toast (history always records it). */
export function shouldToast(n: AppNotification, doNotDisturb: boolean): boolean {
  if (n.read) return false;
  // Critical notices (emergency stop, security) break through Do Not Disturb.
  if (doNotDisturb) return n.level === "Critical";
  return true;
}
