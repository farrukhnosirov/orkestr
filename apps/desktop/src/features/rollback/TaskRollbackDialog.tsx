/**
 * Rollback Task (§25, §63.J): undo only one task's changes and keep everyone
 * else's. Two steps over `snapshots.rollbackTask`: a dry run shows what would
 * be restored / deleted and which files are conflicts (changed by someone
 * else, never overwritten), then an explicit confirmation applies it. The
 * core takes a safety snapshot first, so the rollback itself can be undone.
 */
import "./rollback.css";

import { OrkestrError } from "@orkestr/client";
import type { RollbackConflict, RollbackResult } from "@orkestr/protocol";
import { Badge, Spinner } from "@orkestr/ui";
import { Undo2 } from "lucide-react";
import { useEffect, useState } from "react";

import { getClient } from "../../core/client";
import { t as i18nT, useT } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { ActionDialog } from "../files/Dialog";

export interface TaskRollbackDialogProps {
  projectId: string;
  taskId: string;
  taskTitle: string;
  onClose: () => void;
  /** Called with a user-facing summary once the rollback was applied. */
  onDone: (message: string) => void;
}

const CONFLICT_REASONS: Readonly<Record<string, "shared" | "betweenAttempts" | "uncaptured" | "afterTask">> = {
  "changed during the task by someone else in the shared workspace — not reverted": "shared",
  "changed outside the task between its attempts — not overwritten": "betweenAttempts",
  "not captured by the task's before-checkpoint (ignored or unreadable then), so it may predate the task — not deleted": "uncaptured",
  "changed by someone else after the task — not overwritten": "afterTask",
};

/** The core explains conflicts in English; known explanations are shown translated. */
export function conflictReason(reason: string): string {
  const key = CONFLICT_REASONS[reason];
  return key ? i18nT(`git:taskRollback.conflict.${key}`) : reason;
}

/** Number of files a confirmed rollback would write or delete. */
export function rollbackChanges(plan: RollbackResult): number {
  return plan.restored.length + plan.deleted.length;
}

/** User-facing summary of an applied task rollback. */
export function taskRollbackSummary(title: string, r: RollbackResult): string {
  const counts = { title, restored: r.restored.length, deleted: r.deleted.length };
  return r.conflicts.length
    ? i18nT("git:taskRollback.doneSkipped", { ...counts, skipped: r.conflicts.length })
    : i18nT("git:taskRollback.done", counts);
}

function PathList({ label, tone, paths }: { label: string; tone: "warning" | "danger" | "neutral"; paths: string[] }) {
  if (paths.length === 0) return null;
  return (
    <section className="trb-group">
      <h3 className="trb-group__title">
        <Badge tone={tone}>{paths.length}</Badge> {label}
      </h3>
      <ul className="trb-list">
        {paths.map((p) => (
          <li key={p} className="trb-list__path" title={p}>
            {p}
          </li>
        ))}
      </ul>
    </section>
  );
}

function ConflictList({ conflicts }: { conflicts: RollbackConflict[] }) {
  const { t } = useT("git");
  if (conflicts.length === 0) return null;
  return (
    <section className="trb-group">
      <h3 className="trb-group__title">
        <Badge tone="danger">{conflicts.length}</Badge> {t("taskRollback.conflicts")}
      </h3>
      <ul className="trb-list">
        {conflicts.map((c) => (
          <li key={c.path}>
            <span className="trb-list__path" title={c.path}>
              {c.path}
            </span>
            <span className="trb-list__reason">{conflictReason(c.reason)}</span>
          </li>
        ))}
      </ul>
    </section>
  );
}

export function TaskRollbackDialog({ projectId, taskId, taskTitle, onClose, onDone }: TaskRollbackDialogProps) {
  const { t } = useT(["git", "common"]);
  const [plan, setPlan] = useState<RollbackResult | null>(null);
  const [planError, setPlanError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setPlan(null);
    setPlanError(null);
    getClient()
      .call("snapshots.rollbackTask", { projectId, taskId, dryRun: true, confirm: false })
      .then((p) => !cancelled && setPlan(p))
      .catch((e: unknown) => {
        if (cancelled) return;
        setPlanError(e instanceof OrkestrError && e.code === "not_found" ? t("taskRollback.noCheckpoints") : describeError(e));
      });
    return () => {
      cancelled = true;
    };
  }, [projectId, taskId, t]);

  const changes = plan ? rollbackChanges(plan) : 0;

  const apply = async () => {
    setBusy(true);
    setError(null);
    try {
      const r = await getClient().call("snapshots.rollbackTask", { projectId, taskId, dryRun: false, confirm: true });
      onDone(taskRollbackSummary(taskTitle, r));
    } catch (e) {
      setError(describeError(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <ActionDialog
      title={t("taskRollback.title", { title: taskTitle })}
      subtitle={t("taskRollback.description")}
      icon={<Undo2 size={18} />}
      tone="danger"
      error={error}
      onCancel={() => !busy && onClose()}
      actions={[
        { id: "cancel", label: t("common:actions.cancel"), variant: "ghost", autoFocus: true, onSelect: () => !busy && onClose() },
        {
          id: "confirm",
          label: t("taskRollback.confirm", { count: changes }),
          variant: "danger",
          icon: <Undo2 size={14} />,
          busy,
          disabled: !plan || changes === 0,
          onSelect: () => void apply(),
        },
      ]}
    >
      <div className="trb-body">
        {planError ? (
          <p className="fx-dialog__text" role="alert">
            {planError}
          </p>
        ) : !plan ? (
          <div className="trb-loading">
            <Spinner size={14} /> {t("taskRollback.planning")}
          </div>
        ) : (
          <>
            {changes === 0 && (
              <p className="fx-dialog__text">
                {plan.conflicts.length > 0 ? t("taskRollback.onlyConflicts") : t("taskRollback.nothing")}
              </p>
            )}
            <PathList label={t("taskRollback.restore")} tone="warning" paths={plan.restored} />
            <PathList label={t("taskRollback.delete")} tone="danger" paths={plan.deleted} />
            <ConflictList conflicts={plan.conflicts} />
            <PathList label={t("taskRollback.unchanged")} tone="neutral" paths={plan.unchanged} />
          </>
        )}
      </div>
    </ActionDialog>
  );
}
