import type { TaskPriority, TaskStatus } from "@orkestr/protocol";
import { Badge } from "@orkestr/ui";
import { clsx } from "clsx";
import { ChevronsDown, ChevronsUp, ChevronDown, ChevronUp, Equal, X } from "lucide-react";

import { PRIORITY_TONE, STATUS_TONE, statusColor } from "./model";
import { useTasksUi } from "./store";

export function StatusBadge({ status, className }: { status: TaskStatus; className?: string }) {
  return (
    <Badge tone={STATUS_TONE[status]} className={clsx("tasks-status", className)}>
      <span className="tasks-status__dot" style={{ background: statusColor(status) }} aria-hidden="true" />
      {status}
    </Badge>
  );
}

const PRIORITY_ICON: Record<TaskPriority, typeof ChevronUp> = {
  Critical: ChevronsUp,
  High: ChevronUp,
  Normal: Equal,
  Low: ChevronDown,
  Background: ChevronsDown,
};

export function PriorityChip({ priority, compact = false }: { priority: TaskPriority; compact?: boolean }) {
  const Icon = PRIORITY_ICON[priority];
  return (
    <span className={clsx("tasks-priority", `tasks-priority--${PRIORITY_TONE[priority]}`)} title={`${priority} priority`}>
      <Icon size={12} aria-hidden="true" />
      {compact ? <span className="ok-visually-hidden">{priority}</span> : priority}
    </span>
  );
}

/** Transient notifications of the panel (errors such as dependency cycles). */
export function Toasts() {
  const toasts = useTasksUi((s) => s.toasts);
  const dismiss = useTasksUi((s) => s.dismissToast);
  if (toasts.length === 0) return null;
  return (
    <div className="tasks-toasts" role="status" aria-live="polite">
      {toasts.map((t) => (
        <div key={t.id} className={clsx("tasks-toast", `tasks-toast--${t.tone}`)}>
          <span>{t.message}</span>
          <button type="button" className="tasks-toast__close" aria-label="Dismiss" onClick={() => dismiss(t.id)}>
            <X size={13} />
          </button>
        </div>
      ))}
    </div>
  );
}
