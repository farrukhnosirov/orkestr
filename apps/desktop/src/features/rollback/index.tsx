/** Rollback Task entry points shared by the Tasks drawer, agent card and run report. */
import type { TaskStatus } from "@orkestr/protocol";
import { Button } from "@orkestr/ui";
import { Undo2 } from "lucide-react";
import { useState } from "react";

import { useT } from "../../i18n";
import { TaskRollbackDialog } from "./TaskRollbackDialog";

export { TaskRollbackDialog, conflictReason, rollbackChanges, taskRollbackSummary } from "./TaskRollbackDialog";

/** States in which a task may have changed files and is not mid-run: Rollback Task is offered. */
const ROLLBACK_STATES: ReadonlySet<TaskStatus> = new Set<TaskStatus>([
  "Waiting",
  "Blocked",
  "Review",
  "Testing",
  "Completed",
  "Failed",
  "Cancelled",
  "Interrupted",
]);

/** Whether Rollback Task is offered for a task in `status`. */
export function canRollbackTask(status: TaskStatus): boolean {
  return ROLLBACK_STATES.has(status);
}

export interface TaskRollbackButtonProps {
  projectId: string;
  taskId: string;
  taskTitle: string;
  onDone: (message: string) => void;
  disabled?: boolean;
}

/** "Roll back task…" button that opens the dry-run → confirm dialog. */
export function TaskRollbackButton({ projectId, taskId, taskTitle, onDone, disabled }: TaskRollbackButtonProps) {
  const { t } = useT("git");
  const [open, setOpen] = useState(false);
  return (
    <>
      <Button size="sm" variant="ghost" leftIcon={<Undo2 size={13} />} disabled={disabled} onClick={() => setOpen(true)}>
        {t("taskRollback.action")}
      </Button>
      {open && (
        <TaskRollbackDialog
          projectId={projectId}
          taskId={taskId}
          taskTitle={taskTitle}
          onClose={() => setOpen(false)}
          onDone={(message) => {
            setOpen(false);
            onDone(message);
          }}
        />
      )}
    </>
  );
}
