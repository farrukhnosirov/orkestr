/**
 * Quality pipeline UI (§17, §68.15–16):
 * - `PipelineSection` — the task drawer's "Pipeline" stepper: stages with
 *   status and runs, review / security verdicts, gates, merge outcome, and
 *   the Self-Healing attempts timeline with each failure analysis (failing
 *   tests, compiler errors, findings, relevant code).
 * - `PipelineStages` — compact stage chips for the Manager run card.
 * Data: `pipeline.*` + live `PipelineUpdated` events.
 */
import "./pipeline.css";

import type {
  HealingAttempt,
  HealingOutcome,
  PipelineStatus,
  RunStageBrief,
  StageStatus,
  TaskPipeline,
  TaskStatus,
} from "@orkestr/protocol";
import { Badge, Button, Spinner, type Tone } from "@orkestr/ui";
import { clsx } from "clsx";
import { Check, Circle, CircleDashed, Hourglass, Loader, Minus, Workflow, Wrench, X } from "lucide-react";
import { useState } from "react";

import { useFormat, useT } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { pipelineActions, useTaskPipeline } from "./data";
import { type PipelineT, localizePipelineText } from "./text";

export const STAGE_TONE: Record<StageStatus, Tone> = {
  Pending: "neutral",
  Running: "info",
  Passed: "success",
  Failed: "danger",
  Skipped: "neutral",
  Waiting: "warning",
};

export const PIPELINE_TONE: Record<PipelineStatus, Tone> = {
  Running: "info",
  Healing: "violet",
  AwaitingUser: "warning",
  Escalated: "warning",
  Passed: "success",
  Failed: "danger",
  Cancelled: "neutral",
};

const OUTCOME_TONE: Record<HealingOutcome, Tone> = {
  Queued: "neutral",
  Running: "info",
  Submitted: "info",
  Fixed: "success",
  StillFailing: "danger",
  AgentFailed: "danger",
  Stopped: "neutral",
};

function StageIcon({ status }: { status: StageStatus }) {
  switch (status) {
    case "Passed":
      return <Check size={12} aria-hidden="true" />;
    case "Failed":
      return <X size={12} aria-hidden="true" />;
    case "Running":
      return <Loader size={12} className="pl-spin" aria-hidden="true" />;
    case "Skipped":
      return <Minus size={12} aria-hidden="true" />;
    case "Waiting":
      return <Hourglass size={12} aria-hidden="true" />;
    default:
      return <CircleDashed size={12} aria-hidden="true" />;
  }
}

/** Compact stage chips (Manager run card). */
export function PipelineStages({ stages, status, healing, task }: { stages: RunStageBrief[]; status: PipelineStatus; healing: number; task: string }) {
  const { t } = useT("pipeline");
  return (
    <ol className="pl-chips" aria-label={t("runStages", { task })}>
      {stages.map((s) => (
        <li key={s.kind} className={clsx("pl-chip", `pl-chip--${s.status.toLowerCase()}`)} title={`${t(`stage.${s.kind}`)}: ${t(`stageStatus.${s.status}`)}`}>
          <StageIcon status={s.status} />
          <span>{t(`stage.${s.kind}`)}</span>
        </li>
      ))}
      {healing > 0 && (
        <li className="pl-chip pl-chip--healing" title={t("healing.title")}>
          <Wrench size={11} aria-hidden="true" /> {healing}
        </li>
      )}
      <li className="pl-chip__status">
        <Badge tone={PIPELINE_TONE[status]}>{t(`status.${status}`)}</Badge>
      </li>
    </ol>
  );
}

export type PipelineToast = (tone: "success" | "danger" | "info", message: string) => void;

/** "Pipeline" section of the task drawer. */
export function PipelineSection({ taskId, taskStatus, onToast }: { taskId: string; taskStatus: TaskStatus; onToast: PipelineToast }) {
  const { t } = useT(["pipeline", "common"]);
  const { pipeline, history, live, loading, error, reload } = useTaskPipeline(taskId);
  const [busy, setBusy] = useState(false);
  const canStart = !live && (taskStatus === "Review" || taskStatus === "Testing");

  const start = async () => {
    setBusy(true);
    try {
      await pipelineActions.start(taskId);
      onToast("info", t("started"));
      reload();
    } catch (e) {
      onToast("danger", describeError(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="tasks-section pl-section" aria-label={t("title")}>
      <h4 className="tasks-section__title">
        <Workflow size={13} /> {t("title")}
        {pipeline && <Badge tone={PIPELINE_TONE[pipeline.status]}>{t(`status.${pipeline.status}`)}</Badge>}
        {live && <span className="tasks-muted">{t("live")}</span>}
      </h4>
      {canStart && (
        <div className="tasks-actions">
          <Button size="sm" variant="secondary" loading={busy} onClick={() => void start()}>
            {t("start")}
          </Button>
        </div>
      )}
      {error ? (
        <p className="tasks-error" role="alert">
          {error}{" "}
          <Button size="sm" variant="ghost" onClick={reload}>
            {t("common:actions.retry")}
          </Button>
        </p>
      ) : loading && !pipeline ? (
        <div className="tasks-muted">
          <Spinner size={14} /> {t("loading")}
        </div>
      ) : !pipeline ? (
        <p className="tasks-muted">{t("empty")}</p>
      ) : (
        <>
          <PipelineView pipeline={pipeline} />
          {history.length > 0 && (
            <details className="pl-earlier">
              <summary>{t("earlier", { count: history.length })}</summary>
              {history.map((p) => (
                <PipelineView key={p.id} pipeline={p} compact />
              ))}
            </details>
          )}
        </>
      )}
    </section>
  );
}

function PipelineView({ pipeline, compact = false }: { pipeline: TaskPipeline; compact?: boolean }) {
  const { t } = useT("pipeline");
  const fmt = useFormat();
  const tt = t as unknown as PipelineT;
  const facts: string[] = [];
  if (pipeline.reviewVerdict) facts.push(t("reviewVerdict", { verdict: t(`verdict.${pipeline.reviewVerdict}`) }));
  if (pipeline.securityVerdict) facts.push(t("securityVerdict", { verdict: t(`verdict.${pipeline.securityVerdict}`) }));
  if (pipeline.gates) facts.push(t("gates", { status: t(`gateStatus.${pipeline.gates.overall}`) }));
  if (pipeline.mergeOutcome) facts.push(t("merge", { outcome: localizePipelineText(pipeline.mergeOutcome, tt) }));
  return (
    <div className={clsx("pl-view", compact && "pl-view--compact")}>
      <ol className="pl-stepper" aria-label={t("stagesLabel")}>
        {pipeline.stages.map((s) => (
          <li key={s.kind} className={clsx("pl-step", `pl-step--${s.status.toLowerCase()}`)}>
            <span className="pl-step__icon">
              <StageIcon status={s.status} />
            </span>
            <span className="pl-step__body">
              <span className="pl-step__name">
                {t(`stage.${s.kind}`)} <Badge tone={STAGE_TONE[s.status]}>{t(`stageStatus.${s.status}`)}</Badge>
                {s.runs > 1 && <span className="tasks-muted"> · {t("runs", { count: s.runs })}</span>}
              </span>
              {s.detail && !compact && <span className="pl-step__detail">{localizePipelineText(s.detail, tt)}</span>}
            </span>
          </li>
        ))}
      </ol>
      {pipeline.statusReason && <p className="pl-reason">{localizePipelineText(pipeline.statusReason, tt)}</p>}
      {facts.length > 0 && <p className="tasks-muted pl-facts">{facts.join(" · ")}</p>}
      {!compact && <HealingTimeline pipeline={pipeline} />}
      {!compact && <p className="tasks-muted pl-facts">{fmt.relative(pipeline.startedAt)}</p>}
    </div>
  );
}

function HealingTimeline({ pipeline }: { pipeline: TaskPipeline }) {
  const { t } = useT("pipeline");
  const limits = t("healing.limits", { attempts: pipeline.limits.maxAttempts, mins: pipeline.limits.maxWallMins });
  return (
    <div className="pl-healing">
      <h5 className="pl-healing__title">
        <Wrench size={12} aria-hidden="true" /> {t("healing.title")}
        <span className="tasks-muted"> · {limits}</span>
        {pipeline.grantedAttempts > 0 && <span className="tasks-muted"> · {t("healing.granted", { count: pipeline.grantedAttempts })}</span>}
      </h5>
      {pipeline.healing.length === 0 ? (
        <p className="tasks-muted">{t("healing.none")}</p>
      ) : (
        <ol className="pl-timeline">
          {pipeline.healing.map((a) => (
            <AttemptItem key={a.attempt + a.startedAt} attempt={a} />
          ))}
        </ol>
      )}
    </div>
  );
}

function AttemptItem({ attempt }: { attempt: HealingAttempt }) {
  const { t } = useT("pipeline");
  const a = attempt.analysis;
  return (
    <li className="pl-attempt">
      <div className="pl-attempt__head">
        <Circle size={8} aria-hidden="true" className="pl-attempt__dot" />
        <strong>{t("healing.attempt", { n: attempt.attempt })}</strong>
        <span className="tasks-muted">{t(`stage.${a.stage}`)}</span>
        <Badge tone={OUTCOME_TONE[attempt.outcome]}>{t(`healing.outcome.${attempt.outcome}`)}</Badge>
        {attempt.model && <span className="tasks-muted">{attempt.model.model}</span>}
        {attempt.escalated && <Badge tone="violet">{t("healing.escalated")}</Badge>}
      </div>
      <p className="pl-attempt__summary">{localizePipelineText(a.summary, t as unknown as PipelineT)}</p>
      {attempt.note && <p className="tasks-muted">{attempt.note}</p>}
      {(a.failingTests.length > 0 || a.compilerErrors.length > 0 || a.findings.length > 0 || a.excerpts.length > 0) && (
        <details className="pl-analysis">
          <summary>{t("healing.findings")}</summary>
          {a.failingTests.length > 0 && (
            <>
              <h6>{t("healing.failingTests")}</h6>
              <ul>
                {a.failingTests.map((f, i) => (
                  <li key={i}>
                    <code>{f.name}</code>
                    {f.file && <span className="tasks-muted"> {f.line ? `${f.file}:${f.line}` : f.file}</span>} — {f.message}
                  </li>
                ))}
              </ul>
            </>
          )}
          {a.compilerErrors.length > 0 && (
            <>
              <h6>{t("healing.compilerErrors")}</h6>
              <ul>
                {a.compilerErrors.map((d, i) => (
                  <li key={i}>
                    {d.file && <code>{d.line ? `${d.file}:${d.line}` : d.file}</code>} {d.code && <span className="tasks-muted">[{d.code}]</span>} {d.message}
                  </li>
                ))}
              </ul>
            </>
          )}
          {a.findings.length > 0 && (
            <ul>
              {a.findings.map((f, i) => (
                <li key={i} className="pl-finding">
                  {f}
                </li>
              ))}
            </ul>
          )}
          {a.excerpts.length > 0 && (
            <>
              <h6>{t("healing.excerpts")}</h6>
              {a.excerpts.map((e, i) => (
                <figure key={i} className="pl-excerpt">
                  <figcaption>
                    <code>{e.file}</code> <span className="tasks-muted">{t("healing.lines", { from: e.startLine, to: e.endLine })}</span>
                  </figcaption>
                  <pre>{e.text}</pre>
                </figure>
              ))}
            </>
          )}
        </details>
      )}
    </li>
  );
}
