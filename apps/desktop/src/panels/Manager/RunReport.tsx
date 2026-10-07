/**
 * Final run report (§1 step 13) rendered from the report's structured fields
 * in the UI language. The core's Markdown `summary` is written once, in the
 * language active when the run ended, so the card does not show it; texts
 * that come from agents (summaries, test lines, issues) are shown as written.
 */
import type { ManagerRun, PipelineReport, RunReport as Report } from "@orkestr/protocol";

import { useLanguage, useT } from "../../i18n";
import { runReason } from "../../lib/coreText";
import { roleLabel } from "../Agents/model";
import { formatDuration, formatUsd } from "./model";

const MERGED = /^merged `(.+?)` into `(.+?)`$/;

/** Localized merge outcome of a task (`proposed — awaiting approval`, `merged `x` into `y``). */
export function mergeOutcomeLabel(outcome: string, t: (key: string, opts?: Record<string, unknown>) => string): string {
  const m = MERGED.exec(outcome.trim());
  if (m) return t("manager:report.mergedInto", { branch: m[1], target: m[2] });
  if (outcome.startsWith("proposed")) return t("manager:report.mergeProposed");
  if (outcome.startsWith("nothing to merge")) return t("manager:report.mergeNothing");
  if (outcome.startsWith("review the diff")) return t("manager:report.mergeReview");
  return outcome;
}

function pipelineLine(p: PipelineReport, t: (key: string, opts?: Record<string, unknown>) => string): string {
  const parts = [t(`pipeline:status.${p.status}`)];
  if (p.reviewVerdict) parts.push(t("manager:report.review", { verdict: t(`reviews:verdict.${p.reviewVerdict}`) }));
  if (p.securityVerdict) parts.push(t("manager:report.security", { verdict: t(`reviews:verdict.${p.securityVerdict}`) }));
  if (p.gates) {
    const failed = p.failedGates.map((g) => t(`quality:gate.${g}`)).join(", ");
    parts.push(
      failed
        ? t("manager:report.gatesFailed", { status: t(`quality:gateStatus.${p.gates}`), gates: failed })
        : t("manager:report.gates", { status: t(`quality:gateStatus.${p.gates}`) }),
    );
  }
  if (p.healingAttempts > 0) parts.push(t("manager:report.healing", { count: p.healingAttempts }));
  if (p.mergeOutcome) parts.push(t("manager:report.merge", { outcome: mergeOutcomeLabel(p.mergeOutcome, t) }));
  return parts.join(" · ");
}

export function RunReport({ run, report }: { run: ManagerRun; report: Report }) {
  const { t } = useT(["manager", "common", "pipeline", "reviews", "quality"]);
  const lng = useLanguage();
  const tt = t as unknown as (key: string, opts?: Record<string, unknown>) => string;
  return (
    <div className="mgr-md mgr-report">
      <p>
        <strong>{t(`runStatus.${run.status}`)}</strong>
        {" — "}
        {t("report.headline", {
          completed: report.completed,
          total: run.tasks.length,
          duration: formatDuration(report.durationSecs, lng),
          cost: formatUsd(report.costUsd, lng),
        })}
        {run.statusReason ? ` (${runReason(run.statusReason)})` : ""}
      </p>
      <h3>{t("report.whatWasDone")}</h3>
      <ul>
        {run.tasks.map((task) => (
          <li key={task.taskId}>
            <strong>{task.title}</strong> ({roleLabel(task.role)}) — {t(`common:status.task.${task.status}`)}
            {task.summary ? `: ${task.summary}` : ""}
          </li>
        ))}
      </ul>
      {report.filesChanged.length > 0 && (
        <>
          <h3>{t("report.filesChanged")}</h3>
          <ul>
            {report.filesChanged.slice(0, 40).map((f) => (
              <li key={f}>
                <code>{f}</code>
              </li>
            ))}
          </ul>
        </>
      )}
      {report.tests.length > 0 && (
        <>
          <h3>{t("report.tests")}</h3>
          <ul>
            {report.tests.map((x, i) => (
              <li key={i} data-user-content>
                {x}
              </li>
            ))}
          </ul>
        </>
      )}
      {report.openIssues.length > 0 && (
        <>
          <h3>{t("report.openIssues")}</h3>
          <ul>
            {report.openIssues.slice(0, 20).map((x, i) => (
              <li key={i} data-user-content>
                {x}
              </li>
            ))}
          </ul>
        </>
      )}
      {report.pipelines.length > 0 && (
        <>
          <h3>{t("report.qualityPipeline")}</h3>
          <ul>
            {report.pipelines.map((p) => (
              <li key={p.taskId}>
                <strong>{p.title}</strong> — {pipelineLine(p, tt)}
              </li>
            ))}
          </ul>
        </>
      )}
      {report.mergeProposals.length > 0 && (
        <>
          <h3>{t("report.mergeProposals")}</h3>
          <ul>
            {report.mergeProposals.map((m) => (
              <li key={m.taskId}>
                <code>{m.branch}</code> — {t("report.files", { count: m.filesChanged.length })}
                {m.additions != null && m.deletions != null ? `, +${m.additions} −${m.deletions}` : ""}
                {m.note ? ` · ${mergeOutcomeLabel(m.note, tt)}` : ""}
              </li>
            ))}
          </ul>
        </>
      )}
    </div>
  );
}
