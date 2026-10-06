/**
 * Maps core events to in-app notifications (§33 notification center).
 * `Notification` events are shown as-is; a few important event kinds are
 * turned into notifications so the user notices them even when the
 * relevant panel is closed. Texts are localized in the language active when
 * the event arrives. Pure and unit-tested.
 */
import type { Event, NotificationLevel } from "@orkestr/protocol";

import { formatCurrency, formatDuration, formatPercent, t } from "../../i18n";
import { taskReason } from "../../lib/coreText";
import { budgetScopeText } from "../../panels/Costs/format";
import { useAppStore } from "../../store/app";

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
      return base(ev, a.risk === "Critical" || a.risk === "High" ? "Warning" : "Info", t("notifications:events.approvalNeeded"), a.actionSummary);
    }
    case "BuildFailed":
      return base(
        ev,
        "Error",
        t("notifications:events.buildFailed"),
        ev.payload.summary || t("notifications:events.exitCode", { code: ev.payload.exitCode ?? "?" }),
      );
    case "TaskStatusChanged": {
      const { to, reason: raw } = ev.payload;
      // Waiting for unfinished dependencies is ordinary planning, not a problem to report.
      if (to === "Blocked" && raw === "dependency_pending") return null;
      // Core reasons are codes / English sentences: shown localized, else the localized default.
      const reason = taskReason(raw);
      if (to === "Failed") return base(ev, "Error", t("notifications:events.taskFailed"), reason ?? t("notifications:events.taskFailedBody"));
      if (to === "Blocked") return base(ev, "Warning", t("notifications:events.taskBlocked"), reason ?? t("notifications:events.taskBlockedBody"));
      if (to === "Completed") {
        return base(ev, "Success", t("notifications:events.taskCompleted"), reason ?? t("notifications:events.taskCompletedBody"));
      }
      return null;
    }
    case "BudgetThreshold": {
      const p = ev.payload;
      return base(
        ev,
        p.fraction >= 1 ? "Error" : "Warning",
        t("notifications:events.budget", { percent: formatPercent(p.fraction) }),
        t("notifications:events.budgetBody", {
          scope: budgetScopeText(p.scope, useAppStore.getState().projects),
          spent: formatCurrency(p.spentUsd),
          limit: formatCurrency(p.limitUsd),
        }),
      );
    }
    case "EmergencyStop":
      return base(
        ev,
        "Critical",
        t("notifications:events.emergencyStop"),
        t("notifications:events.agentsStopped", { count: ev.payload.agentsStopped }),
      );
    case "ProviderRateLimited": {
      const p = ev.payload;
      const what = p.model
        ? t("notifications:events.rateLimitedModel", { model: p.model })
        : t("notifications:events.rateLimitedUnknown");
      const retry =
        p.retryAfterSecs != null ? ` ${t("notifications:events.retryIn", { delay: formatDuration(p.retryAfterSecs * 1000) })}` : "";
      return base(ev, "Warning", t("notifications:events.rateLimited"), `${what}${retry}`);
    }
    case "ProviderFallback": {
      const p = ev.payload;
      return base(
        ev,
        "Warning",
        t("notifications:events.providerFallback"),
        t("notifications:events.providerFallbackBody", {
          from: p.from.model,
          to: p.to.model,
          reason: t(`notifications:fallbackReason.${p.reason}`, { defaultValue: p.reason }),
        }),
      );
    }
    case "RollbackPerformed":
      return base(
        ev,
        "Info",
        t("notifications:events.rollback"),
        t("notifications:events.filesRestored", { count: ev.payload.filesRestored }),
      );
    case "NodeOffline":
      return base(ev, "Warning", t("notifications:events.nodeOffline"), t("notifications:events.nodeOfflineBody"));
    default:
      return null;
  }
}

/** Whether a notification should pop up as a toast (history always records it). */
export function shouldToast(n: AppNotification, doNotDisturb: boolean): boolean {
  if (n.read) return false;
  // Approvals already pop up as the actionable security toast (Review /
  // Approve); a second, passive toast for the same request only stacks up.
  if (n.source === "ApprovalRequested") return false;
  // Critical notices (emergency stop, security) break through Do Not Disturb.
  if (doNotDisturb) return n.level === "Critical";
  return true;
}
