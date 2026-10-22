/**
 * Jobs placed on nodes (`jobs.list`): state, node, progress, the scheduler's
 * placement explanation, and Migrate (choose target) / Cancel.
 */
import type { JobView, NodeSnapshot } from "@orkestr/protocol";
import { Badge, Button } from "@orkestr/ui";
import { useState } from "react";

import { getClient } from "../../core/client";
import { useT } from "../../i18n";
import { formatDate, formatPercent } from "../../i18n/format";
import { isTerminalJob, jobNodeId, jobTone } from "./logic";
import { NoticeLine, useAction } from "./useAction";

const AUTO = "";

function JobRow({ job, nodes, onChanged }: { job: JobView; nodes: NodeSnapshot[]; onChanged: () => Promise<void> }) {
  const { t } = useT("nodes");
  const { t: tc } = useT("common");
  const { busy, notice, run } = useAction(onChanged);
  const [migrating, setMigrating] = useState(false);
  const [target, setTarget] = useState(AUTO);
  const id = job.spec.jobId;
  const nodeId = jobNodeId(job);
  const nodeName = (nid: string | null) => (nid ? (nodes.find((s) => s.node.id === nid)?.node.name ?? nid.slice(0, 8)) : t("jobs.unassigned"));
  const targets = nodes.filter((s) => s.node.id !== nodeId && s.status !== "Revoked" && s.status !== "Draining");
  const explanation = job.placement?.explanation ?? [];
  const kind = job.spec.kind;
  const summary =
    kind.type === "command" ? [kind.program, ...kind.args].join(" ") : kind.type === "agentTask" ? kind.prompt : t("jobs.kind.service");

  const migrate = () =>
    run("migrate", async () => {
      const r = await getClient().call("jobs.migrate", { jobId: id, ...(target ? { targetNodeId: target } : {}) });
      setMigrating(false);
      return t("jobs.migrated", { node: nodeName(jobNodeId(r.job)) });
    });
  const cancel = () =>
    run("cancel", async () => {
      await getClient().call("jobs.cancel", { jobId: id, immediate: false });
      return t("jobs.cancelled");
    });

  return (
    <li className="nd-card" data-testid={`job-${id}`}>
      <div className="nd-card__head">
        <Badge tone={jobTone(job.state)} data-testid={`job-state-${id}`}>
          {t(`jobState.${job.state}`)}
        </Badge>
        <Badge tone="neutral">{t(`jobs.kind.${kind.type}`)}</Badge>
        <strong className="nd-card__name" title={summary}>
          {summary}
        </strong>
        <span className="nd-muted" data-testid={`job-node-${id}`}>
          {t("jobs.on", { node: nodeName(nodeId) })}
        </span>
        {job.progress != null && <span className="nd-muted">{formatPercent(job.progress)}</span>}
        <span className="nd-muted">{t("node.epoch", { epoch: job.epoch })}</span>
        <span className="nd-muted">{formatDate(job.updatedAt)}</span>
      </div>
      {job.detail && <p className="nd-muted">{job.detail === "needs_user_decision" ? t("jobs.needsDecision") : job.detail}</p>}
      {explanation.length > 0 && (
        <details className="nd-explain">
          <summary>{t("jobs.why")}</summary>
          <ul data-testid={`job-explanation-${id}`}>
            {explanation.map((line, i) => (
              <li key={i}>{line}</li>
            ))}
          </ul>
        </details>
      )}
      {!isTerminalJob(job) && (
        <div className="nd-actions">
          {migrating ? (
            <>
              <label className="nd-select">
                <span>{t("jobs.target")}</span>
                <select value={target} onChange={(e) => setTarget(e.target.value)} data-testid={`job-target-${id}`}>
                  <option value={AUTO}>{t("jobs.auto")}</option>
                  {targets.map((s) => (
                    <option key={s.node.id} value={s.node.id}>
                      {s.node.name}
                    </option>
                  ))}
                </select>
              </label>
              <Button size="sm" variant="primary" loading={busy === "migrate"} disabled={busy !== null} onClick={() => void migrate()} data-testid={`job-migrate-confirm-${id}`}>
                {t("jobs.migrate")}
              </Button>
              <Button size="sm" disabled={busy !== null} onClick={() => setMigrating(false)}>
                {tc("actions.cancel")}
              </Button>
            </>
          ) : (
            <Button size="sm" disabled={busy !== null} onClick={() => setMigrating(true)} data-testid={`job-migrate-${id}`}>
              {t("jobs.migrate")}
            </Button>
          )}
          <Button size="sm" variant="danger" loading={busy === "cancel"} disabled={busy !== null} onClick={() => void cancel()} data-testid={`job-cancel-${id}`}>
            {t("jobs.cancel")}
          </Button>
        </div>
      )}
      <NoticeLine notice={notice} />
    </li>
  );
}

export function JobsTab({
  jobs,
  unavailable,
  nodes,
  onChanged,
}: {
  jobs: JobView[];
  unavailable: boolean;
  nodes: NodeSnapshot[];
  onChanged: () => Promise<void>;
}) {
  const { t } = useT("nodes");
  if (unavailable)
    return (
      <p className="nd-muted" role="status">
        {t("jobs.unavailable")}
      </p>
    );
  if (jobs.length === 0) return <p className="nd-muted">{t("jobs.none")}</p>;
  return (
    <ul className="nd-list" aria-label={t("tabs.jobs")}>
      {jobs.map((j) => (
        <JobRow key={j.spec.jobId} job={j} nodes={nodes} onChanged={onChanged} />
      ))}
    </ul>
  );
}
