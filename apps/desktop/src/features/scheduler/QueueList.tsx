/**
 * Scheduler entries (running + queued/waiting) with localized wait reasons
 * and an inline "Why?" explanation (`scheduler.explain`, check by check).
 */
import type { SchedEntry, SchedulerExplanation, SchedulerStatus } from "@orkestr/protocol";
import { Badge, Button, Spinner } from "@orkestr/ui";
import { CheckCircle2, CircleSlash, HelpCircle } from "lucide-react";
import { useEffect, useState } from "react";

import { useFormat, useT } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { builtinTeamName } from "../../lib/coreText";
import { schedulerActions } from "./data";
import { entryWaitText, modelName, providerName, reasonText } from "./model";

const STATE_TONE = { running: "info", queued: "accent", waiting: "warning" } as const;

function Explanation({ taskId, status }: { taskId: string; status: SchedulerStatus | null }) {
  const { t } = useT(["scheduler", "common"]);
  const [data, setData] = useState<SchedulerExplanation | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    schedulerActions
      .explain(taskId)
      .then((d) => {
        if (!cancelled) setData(d);
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(describeError(e));
      });
    return () => {
      cancelled = true;
    };
  }, [taskId]);
  if (error) {
    return (
      <p className="sched-error" role="alert">
        {t("queue.explainFailed")}: {error}
      </p>
    );
  }
  if (!data) {
    return (
      <p className="sched-muted">
        <Spinner size={12} /> {t("queue.explainLoading")}
      </p>
    );
  }
  return (
    <div className="sched-explain">
      {!data.scheduled && <p className="sched-muted">{t("queue.notScheduled")}</p>}
      <ul className="sched-checks">
        {data.checks.map((c) => (
          <li key={c.check} className={c.ok ? "sched-check sched-check--ok" : "sched-check sched-check--fail"}>
            {c.ok ? <CheckCircle2 size={13} /> : <CircleSlash size={13} />}
            <span className="sched-check__name">{t(`check.${c.check}` as "check.global", { defaultValue: c.check })}</span>
            <span className="sched-check__text">{c.ok || !c.reason ? t("queue.ok") : reasonText(c.reason, status)}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}

export function QueueRow({
  entry,
  status,
  projectId,
}: {
  entry: SchedEntry;
  status: SchedulerStatus | null;
  projectId: string | null;
}) {
  const { t } = useT(["scheduler", "tasks", "common"]);
  const fmt = useFormat();
  const [open, setOpen] = useState(false);
  const wait = entryWaitText(entry, status);
  const aged = entry.effectivePriority !== entry.priority;
  return (
    <li className="sched-row" data-state={entry.state}>
      <div className="sched-row__main">
        {entry.position !== null && entry.state !== "running" && (
          <span className="sched-row__pos">{t("queue.position", { position: entry.position })}</span>
        )}
        <span className="sched-row__title" title={entry.title}>
          {entry.title}
        </span>
        <Badge tone={STATE_TONE[entry.state]}>{t(`state.${entry.state}`)}</Badge>
        <Badge title={aged ? t("queue.aged", { from: t(`tasks:priority.${entry.priority}`) }) : undefined}>
          {t(`tasks:priority.${entry.effectivePriority}`)}
          {aged ? " ↑" : ""}
        </Badge>
        {projectId && entry.projectId !== projectId && <Badge tone="violet">{t("queue.otherProject")}</Badge>}
        <span className="sched-row__spacer" />
        {entry.state !== "running" && (
          <Button
            size="sm"
            variant="ghost"
            leftIcon={<HelpCircle size={13} />}
            aria-expanded={open}
            onClick={() => setOpen((o) => !o)}
          >
            {t("queue.explain")}
          </Button>
        )}
      </div>
      <div className="sched-row__meta">
        {entry.providerId && <span>{providerName(status, entry.providerId)}</span>}
        {entry.model && <span className="sched-mono">{modelName(entry.model)}</span>}
        {entry.team && <span>{builtinTeamName(entry.team)}</span>}
        <span>{t("queue.since", { time: fmt.relative(entry.since) })}</span>
      </div>
      {wait && <p className="sched-row__reason">{wait}</p>}
      {open && <Explanation taskId={entry.taskId} status={status} />}
    </li>
  );
}

export function QueueList({
  entries,
  status,
  projectId,
}: {
  entries: readonly SchedEntry[];
  status: SchedulerStatus | null;
  projectId: string | null;
}) {
  return (
    <ul className="sched-list">
      {entries.map((e) => (
        <QueueRow key={e.taskId} entry={e} status={status} projectId={projectId} />
      ))}
    </ul>
  );
}
