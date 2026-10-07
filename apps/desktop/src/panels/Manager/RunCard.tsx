/**
 * Live run card (§1 steps 5–13, §11): progress bar, per-task status with the
 * agent working on it (link to the Agents panel), cost so far, Pause /
 * Resume / Cancel, review decisions for results that wait for the user, and
 * the quality pipeline's stages per task (Phase 2) and the final report
 * with merge proposals.
 */
import type { ManagerRun, RunTask } from "@orkestr/protocol";
import { Badge, Button } from "@orkestr/ui";
import { clsx } from "clsx";
import { Bot, Check, GitBranch, OctagonX, Pause, Play, Rocket, X } from "lucide-react";
import { useState } from "react";

import { workspaceActions } from "../../components/workspaceActions";
import { PipelineStages } from "../../features/pipeline";
import { useLanguage, useT } from "../../i18n";
import { runReason, runTaskError } from "../../lib/coreText";
import { roleLabel } from "../Agents/model";
import { useAgentsUi } from "../Agents/store";
import { STATUS_TONE } from "../Tasks/model";
import { errorMessage, managerActions } from "./data";
import { RUN_STATUS_TONE, formatDuration, formatUsd, isRunFinal, runProgress } from "./model";
import { RunReport } from "./RunReport";
import { useManagerUi } from "./store";

/** Opens the Agents panel on an agent. */
export function openAgent(agentId: string): void {
  useAgentsUi.getState().setTab("agents");
  useAgentsUi.getState().select(agentId);
  workspaceActions.openPanel("agents");
}

function TaskRow({ task, live }: { task: RunTask; live: boolean }) {
  const { t } = useT(["manager", "common"]);
  const lng = useLanguage();
  const toast = useManagerUi((s) => s.toast);
  const [busy, setBusy] = useState(false);
  const review = async (accept: boolean) => {
    setBusy(true);
    try {
      await managerActions.reviewTask(task.taskId, accept);
    } catch (e) {
      toast("danger", errorMessage(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <li className={clsx("mgr-rtask", `mgr-rtask--${task.status.toLowerCase()}`)}>
      <div className="mgr-rtask__row">
        <span className={clsx("mgr-rtask__dot", live && "mgr-rtask__dot--live")} aria-hidden="true" />
        <span className="mgr-rtask__title">{task.title}</span>
        <span className="mgr-muted">{roleLabel(task.role)}</span>
        <Badge tone={task.awaitingReview ? "warning" : STATUS_TONE[task.status]}>
          {task.awaitingReview ? t("run.needsReview") : t(`common:status.task.${task.status}`)}
        </Badge>
        {task.agentId && (
          <button type="button" className="mgr-link" onClick={() => openAgent(task.agentId ?? "")} aria-label={t("run.openAgent", { task: task.title })}>
            <Bot size={12} aria-hidden="true" /> {t("run.agent")}
          </button>
        )}
        {task.costUsd > 0 && <span className="mgr-muted">{formatUsd(task.costUsd, lng)}</span>}
      </div>
      {task.branch && (
        <div className="mgr-rtask__meta">
          <GitBranch size={11} aria-hidden="true" /> {task.branch}
          {task.attempts > 1 && <span> · {t("run.attempt", { n: task.attempts })}</span>}
        </div>
      )}
      {task.pipeline && (
        <PipelineStages
          stages={task.pipeline.stages}
          status={task.pipeline.status}
          healing={task.pipeline.healingAttempts}
          task={task.title}
        />
      )}
      {task.lastError && <div className="mgr-rtask__meta mgr-rtask__meta--warn">{runTaskError(task.lastError)}</div>}
      {task.awaitingReview && (
        <div className="mgr-rtask__review">
          {task.summary && <span className="mgr-muted">{task.summary}</span>}
          <Button size="sm" variant="primary" leftIcon={<Check size={13} />} loading={busy} onClick={() => void review(true)}>
            {t("run.accept")}
          </Button>
          <Button size="sm" variant="ghost" leftIcon={<X size={13} />} disabled={busy} onClick={() => void review(false)}>
            {t("run.rejectRetry")}
          </Button>
        </div>
      )}
    </li>
  );
}

export function RunCard({ run, compact = false }: { run: ManagerRun; compact?: boolean }) {
  const { t } = useT(["manager", "common"]);
  const lng = useLanguage();
  const toast = useManagerUi((s) => s.toast);
  const [busy, setBusy] = useState<string | null>(null);
  const progress = runProgress(run);
  const final = isRunFinal(run.status);
  const budget = run.limits.maxCostUsd;
  const liveTasks = run.tasks.filter((x) => x.status === "Running" || x.status === "Waiting");
  const cost = budget !== null ? t("run.costOfBudget", { cost: formatUsd(run.costUsd, lng), budget: formatUsd(budget, lng) }) : formatUsd(run.costUsd, lng);
  const stats = [
    t("run.done", { completed: progress.completed, total: progress.total }),
    ...(progress.running > 0 ? [t("run.running", { count: progress.running })] : []),
    ...(progress.review > 0 ? [t("run.review", { count: progress.review })] : []),
    ...(progress.failed > 0 ? [t("run.failed", { count: progress.failed })] : []),
    cost,
    formatDuration(run.activeSecs, lng),
  ];

  const act = async (what: "pause" | "resume" | "cancel") => {
    setBusy(what);
    try {
      await managerActions[what](run.id);
    } catch (e) {
      toast("danger", errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <section className={clsx("mgr-run", compact && "mgr-run--compact", `mgr-run--${run.status.toLowerCase()}`)} aria-label={t("run.region", { goal: run.goal })}>
      <header className="mgr-run__head">
        <Rocket size={15} aria-hidden="true" />
        <div className="mgr-run__title">
          <strong>{compact ? t("run.inProgress") : run.goal}</strong>
          <span className="mgr-muted">{stats.join(" · ")}</span>
        </div>
        <Badge tone={RUN_STATUS_TONE[run.status]}>{t(`runStatus.${run.status}`)}</Badge>
        {!final && (
          <div className="mgr-run__actions">
            {run.status === "Running" && (
              <Button size="sm" variant="ghost" leftIcon={<Pause size={13} />} loading={busy === "pause"} onClick={() => void act("pause")}>
                {t("run.pause")}
              </Button>
            )}
            {(run.status === "Paused" || run.status === "Interrupted") && (
              <Button size="sm" variant="ghost" leftIcon={<Play size={13} />} loading={busy === "resume"} onClick={() => void act("resume")}>
                {t("run.resume")}
              </Button>
            )}
            <Button size="sm" variant="danger" leftIcon={<OctagonX size={13} />} loading={busy === "cancel"} onClick={() => void act("cancel")}>
              {t("common:actions.cancel")}
            </Button>
          </div>
        )}
      </header>
      <div
        className="mgr-progress"
        role="progressbar"
        aria-label={t("run.progress")}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={progress.percent}
      >
        <div className={clsx("mgr-progress__bar", !final && "mgr-progress__bar--live")} style={{ width: `${progress.percent}%` }} />
      </div>
      {run.statusReason && <p className="mgr-run__reason">{runReason(run.statusReason)}</p>}
      {compact ? (
        liveTasks.length > 0 && (
          <div className="mgr-run__agents">
            {liveTasks.map((x) =>
              x.agentId ? (
                <button key={x.taskId} type="button" className="mgr-chip" onClick={() => openAgent(x.agentId ?? "")}>
                  <Bot size={12} aria-hidden="true" /> {roleLabel(x.role)} — {x.title}
                </button>
              ) : null,
            )}
          </div>
        )
      ) : (
        <ul className="mgr-run__tasks">
          {run.tasks.map((x) => (
            <TaskRow key={x.taskId} task={x} live={!final && (x.status === "Running" || x.status === "Waiting")} />
          ))}
        </ul>
      )}
      {!compact && run.report && (
        <details className="mgr-run__report" open={final}>
          <summary>{t("run.report")}</summary>
          <RunReport run={run} report={run.report} />
        </details>
      )}
    </section>
  );
}
