/**
 * "Queue" view of the Agents panel (§11, §12): every agent slot the global
 * scheduler holds — running now and queued / waiting with reasons — across
 * all projects (runs continue in the background whichever project is open),
 * plus provider load and host resources.
 */
import "./scheduler.css";

import { Button, EmptyState, Spinner } from "@orkestr/ui";
import { ListOrdered, RefreshCw, TriangleAlert } from "lucide-react";

import { useFormat, useT } from "../../i18n";
import { useSchedulerStatus } from "./data";
import { shortId } from "./model";
import { QueueList } from "./QueueList";

const MB = 1024 * 1024;

export function QueueView({ projectId }: { projectId: string | null }) {
  const { t } = useT(["scheduler", "common"]);
  const fmt = useFormat();
  const { status, loading, error, reload } = useSchedulerStatus(null);

  if (!status && loading) {
    return (
      <div className="sched-center">
        <Spinner size={18} /> {t("queue.loading")}
      </div>
    );
  }
  if (!status) {
    return (
      <EmptyState
        icon={<TriangleAlert size={26} strokeWidth={1.6} />}
        title={t("queue.loadFailed")}
        description={error ?? undefined}
        action={
          <Button leftIcon={<RefreshCw size={14} />} onClick={reload}>
            {t("common:actions.retry")}
          </Button>
        }
      />
    );
  }

  const r = status.resources;
  const busyProviders = status.providers.filter((p) => p.runningAgents > 0 || p.inFlight > 0 || p.queuedRequests > 0);

  return (
    <div className="sched-queue">
      <h3 className="sched-queue__title">{t("queue.title")}</h3>
      {status.running.length === 0 && status.queue.length === 0 ? (
        <EmptyState
          icon={<ListOrdered size={26} strokeWidth={1.6} />}
          title={t("queue.empty.title")}
          description={t("queue.empty.description")}
        />
      ) : (
        <>
          {status.running.length > 0 && (
            <section>
              <h4 className="sched-queue__section">{t("queue.running")}</h4>
              <QueueList entries={status.running} status={status} projectId={projectId} />
            </section>
          )}
          {status.queue.length > 0 && (
            <section>
              <h4 className="sched-queue__section">{t("queue.waiting")}</h4>
              <QueueList entries={status.queue} status={status} projectId={projectId} />
            </section>
          )}
        </>
      )}
      {busyProviders.length > 0 && (
        <section>
          <h4 className="sched-queue__section">{t("queue.providers")}</h4>
          <ul className="sched-plain">
            {busyProviders.map((p) => (
              <li key={p.providerId}>
                {t("queue.providerLine", {
                  name: p.name ?? shortId(p.providerId),
                  running: p.runningAgents,
                  inFlight: p.inFlight,
                })}
                {p.agentLimit !== null && <> · {t("queue.providerLimit", { limit: p.agentLimit })}</>}
              </li>
            ))}
          </ul>
        </section>
      )}
      <p className="sched-muted">
        {r.sampled
          ? t("queue.resources", { cpu: fmt.percent(r.cpuPercent / 100), ram: fmt.bytes(r.ramAvailableMb * MB) })
          : t("bar.notSampled")}
        {r.sampled && r.diskFreeMb !== null && <> · {t("queue.resourcesDisk", { disk: fmt.bytes(r.diskFreeMb * MB) })}</>}
      </p>
    </div>
  );
}
