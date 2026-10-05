/**
 * Scheduler bar of the Tasks panel (§11, §68.24–26): running / queued /
 * waiting work of the project with the main wait reason ("Waiting: provider
 * concurrency 2/2"), global capacity, providers in rate-limit back-off,
 * early merge-conflict warnings, an expandable queue and the limits editor.
 */
import "./scheduler.css";

import { Badge, Button, IconButton, Spinner } from "@orkestr/ui";
import { ChevronDown, ChevronUp, Gauge, GitMerge, SlidersHorizontal, Timer } from "lucide-react";
import { useState } from "react";

import { useFormat, useT } from "../../i18n";
import { useSchedulerStatus } from "./data";
import { LimitsDialog } from "./LimitsDialog";
import { rateLimitedProviders, reasonText, topReason } from "./model";
import { QueueList } from "./QueueList";

export function SchedulerBar({ projectId, titles }: { projectId: string; titles?: ReadonlyMap<string, string> }) {
  const { t } = useT(["scheduler", "common"]);
  const fmt = useFormat();
  const { status, loading, error, reload } = useSchedulerStatus(projectId);
  const [showQueue, setShowQueue] = useState(false);
  const [limitsOpen, setLimitsOpen] = useState(false);
  const [saved, setSaved] = useState(false);

  if (!status) {
    return (
      <div className="sched-bar" role="status" aria-label={t("bar.label")}>
        <Gauge size={14} className="sched-bar__icon" />
        {loading ? <Spinner size={12} /> : <span className="sched-muted">{error ? t("bar.loadFailed") : t("bar.idle")}</span>}
      </div>
    );
  }

  const running = status.running.length;
  const queued = status.queue.filter((e) => e.state === "queued").length;
  const waiting = status.queue.filter((e) => e.state === "waiting").length;
  const top = topReason(status.queue);
  const limited = rateLimitedProviders(status);
  const project = status.projects.find((p) => p.projectId === projectId);
  const globalRunning = status.counts.running;
  const title = (id: string) => titles?.get(id) ?? status.queue.concat(status.running).find((e) => e.taskId === id)?.title ?? id;
  const risks = status.mergeRisks;
  const riskTooltip = risks
    .map((r) =>
      r.conflicts.length > 0
        ? t("bar.mergeRiskConfirmed", { files: fmt.list(r.conflicts.slice(0, 5)) })
        : t("bar.mergeRiskDetail", { a: title(r.taskA), b: title(r.taskB), files: fmt.list(r.files.slice(0, 5)) }),
    )
    .join("\n");
  const idle = running === 0 && queued === 0 && waiting === 0;

  return (
    <div className="sched-bar-wrap">
      <div className="sched-bar" role="status" aria-label={t("bar.label")}>
        <Gauge size={14} className="sched-bar__icon" />
        {idle ? (
          <span className="sched-muted">{t("bar.idle")}</span>
        ) : (
          <>
            {running > 0 && <Badge tone="info">{t("bar.running", { count: running })}</Badge>}
            {queued > 0 && <Badge tone="accent">{t("bar.queued", { count: queued })}</Badge>}
            {waiting > 0 && <Badge tone="warning">{t("bar.waiting", { count: waiting })}</Badge>}
          </>
        )}
        {top && (
          <span className="sched-bar__reason" title={t("reason.waiting", { reason: reasonText(top, status) })}>
            {t("reason.waiting", { reason: reasonText(top, status) })}
          </span>
        )}
        {limited.map((p) => (
          <Badge key={p.providerId} tone="danger" icon={<Timer size={12} />}>
            {t("bar.rateLimited", {
              provider: p.name ?? p.providerId,
              time: fmt.date(p.rateLimitedUntil ?? status.updatedAt, "time"),
            })}
          </Badge>
        ))}
        {risks.length > 0 && (
          <Badge tone="warning" icon={<GitMerge size={12} />} title={riskTooltip}>
            {t("bar.mergeRisks", { count: risks.length })}
          </Badge>
        )}
        <span className="sched-bar__spacer" />
        <span className="sched-bar__cap">
          {t("bar.capacity", { running: globalRunning, limit: status.settings.maxParallelGlobal })}
        </span>
        {project && (
          <span className="sched-bar__cap">{t("bar.project", { running: project.running, limit: project.limit })}</span>
        )}
        {saved && <span className="sched-muted">{t("limits.saved")}</span>}
        {(queued > 0 || waiting > 0 || running > 0) && (
          <IconButton
            label={showQueue ? t("bar.hideQueue") : t("bar.showQueue")}
            icon={showQueue ? <ChevronUp size={14} /> : <ChevronDown size={14} />}
            aria-expanded={showQueue}
            onClick={() => setShowQueue((s) => !s)}
            tooltip="bottom"
          />
        )}
        <Button
          size="sm"
          variant="ghost"
          leftIcon={<SlidersHorizontal size={13} />}
          title={t("bar.limitsTooltip")}
          onClick={() => setLimitsOpen(true)}
        >
          {t("bar.limits")}
        </Button>
      </div>
      {showQueue && (
        <div className="sched-bar__queue">
          <QueueList entries={[...status.running, ...status.queue]} status={status} projectId={projectId} />
        </div>
      )}
      {limitsOpen && (
        <LimitsDialog
          status={status}
          projectId={projectId}
          onClose={() => setLimitsOpen(false)}
          onSaved={() => {
            setSaved(true);
            window.setTimeout(() => setSaved(false), 3000);
            reload();
          }}
        />
      )}
    </div>
  );
}
