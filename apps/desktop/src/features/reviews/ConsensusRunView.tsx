/**
 * One consensus run: side-by-side proposals (chosen first, scored by the
 * synthesizer) and the synthesized decision (§8, §68.13).
 */
import type { ConsensusProposal, ConsensusRun } from "@orkestr/protocol";
import { Badge, Button, Spinner } from "@orkestr/ui";
import { clsx } from "clsx";
import { BookMarked, Crown, Scale } from "lucide-react";

import { Markdown } from "../../panels/Memory/markdown";
import { useFormat, useT } from "../../i18n";
import { workspaceActions } from "../../components/workspaceActions";
import { modelLabel, orderedProposals } from "./model";

export function ConsensusRunView({ run }: { run: ConsensusRun }) {
  const { t } = useT("consensus");
  const fmt = useFormat();
  const active = run.status === "Proposing" || run.status === "Synthesizing";
  const chosen = run.decision?.chosenIndex ?? null;
  return (
    <div className="cs-run">
      <div className="cs-run__head">
        <Scale size={15} aria-hidden="true" />
        <p className="cs-run__question">{run.question}</p>
        <Badge tone={run.status === "Finished" ? "success" : run.status === "Failed" ? "danger" : "accent"}>
          {active && <Spinner size={11} />} {t(`status.${run.status}`)}
        </Badge>
      </div>
      {run.status === "Failed" && run.error && (
        <p className="tasks-error" role="alert">
          {t("failedRun", { error: run.error })}
        </p>
      )}

      {run.decision && (
        <section className="cs-decision" aria-label={t("decision")}>
          <header className="cs-decision__head">
            <h3>{run.decision.title}</h3>
            <Badge tone="info">{t("agreement", { value: fmt.percent(run.decision.agreement) })}</Badge>
          </header>
          <Markdown source={run.decision.decision} className="md" />
          {run.decision.reasoningSummary && (
            <>
              <h4>{t("reasoning")}</h4>
              <Markdown source={run.decision.reasoningSummary} />
            </>
          )}
          {run.decision.consequences && (
            <>
              <h4>{t("consequences")}</h4>
              <Markdown source={run.decision.consequences} />
            </>
          )}
          <footer className="cs-decision__foot">
            {run.synthesizer && <span>{t("synthesizedBy", { model: modelLabel(run.synthesizer) })}</span>}
            {run.usage.costUsd > 0 && <span>{t("cost", { cost: fmt.currency(run.usage.costUsd) })}</span>}
            {run.decisionId && (
              <Button size="sm" variant="ghost" leftIcon={<BookMarked size={13} />} onClick={() => workspaceActions.openPanel("memory")}>
                {t("openDecisionLog")}
              </Button>
            )}
          </footer>
        </section>
      )}

      <h4 className="cs-subtitle">{t("proposals")}</h4>
      <div className="cs-proposals" role="list">
        {orderedProposals(run).map((p) => (
          <ProposalCard key={p.index} proposal={p} chosen={p.index === chosen} />
        ))}
      </div>

      {run.warnings.length > 0 && (
        <div className="cs-warnings">
          <h4 className="cs-subtitle">{t("warnings")}</h4>
          <ul>
            {run.warnings.map((w, i) => (
              <li key={i}>{w}</li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}

function List({ title, items, tone }: { title: string; items: string[]; tone: "ok" | "bad" | "warn" }) {
  if (items.length === 0) return null;
  return (
    <div className={clsx("cs-list", `cs-list--${tone}`)}>
      <span>{title}</span>
      <ul>
        {items.map((x, i) => (
          <li key={i}>{x}</li>
        ))}
      </ul>
    </div>
  );
}

function ProposalCard({ proposal: p, chosen }: { proposal: ConsensusProposal; chosen: boolean }) {
  const { t } = useT("consensus");
  const fmt = useFormat();
  return (
    <article role="listitem" className={clsx("cs-proposal", chosen && "cs-proposal--chosen", p.status === "Failed" && "cs-proposal--failed")}>
      <header className="cs-proposal__head">
        <span className="cs-proposal__model" title={p.model.providerId}>
          {modelLabel(p.answeredBy ?? p.model)}
        </span>
        {chosen && (
          <Badge tone="success" icon={<Crown size={11} />}>
            {t("chosen")}
          </Badge>
        )}
        {p.score !== null && (
          <span className="cs-score" title={t("scoreLabel")}>
            {t("score", { score: fmt.number(p.score) })}
          </span>
        )}
      </header>
      {p.status === "Pending" ? (
        <p className="rv-muted rv-progress">
          <Spinner size={13} /> {t("proposalStatus.Pending")}
        </p>
      ) : p.status === "Failed" ? (
        <p className="tasks-error">{p.error ?? t("proposalStatus.Failed")}</p>
      ) : (
        <>
          {p.title && <h4 className="cs-proposal__title">{p.title}</h4>}
          {p.summary && <p className="cs-proposal__summary">{p.summary}</p>}
          <div className="rv-meta">
            <span>{t("confidence", { value: fmt.percent(p.confidence) })}</span>
            {p.answeredBy && (p.answeredBy.model !== p.model.model || p.answeredBy.providerId !== p.model.providerId) && (
              <span>{t("answeredBy", { model: modelLabel(p.answeredBy) })}</span>
            )}
          </div>
          {p.scoreNote && <p className="rv-note">{p.scoreNote}</p>}
          <List title={t("pros")} items={p.pros} tone="ok" />
          <List title={t("cons")} items={p.cons} tone="bad" />
          <List title={t("risks")} items={p.risks} tone="warn" />
          {p.approach && (
            <details className="cs-approach">
              <summary>{t("approach")}</summary>
              <Markdown source={p.approach} />
            </details>
          )}
        </>
      )}
    </article>
  );
}
