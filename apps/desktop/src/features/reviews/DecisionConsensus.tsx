/**
 * Decision Log add-on (Memory → Decision Log): for a decision produced by
 * Consensus Mode, shows that it came from N models, the agreement and every
 * alternative with its model, synthesizer score and whether it was chosen.
 */
import "./reviews.css";

import type { ConsensusRun } from "@orkestr/protocol";
import { Badge } from "@orkestr/ui";
import { clsx } from "clsx";
import { Crown, Scale } from "lucide-react";

import { useFormat, useT } from "../../i18n";
import { modelLabel, orderedProposals } from "./model";

/** Map of Decision Log entry id → the consensus run that recorded it. */
export function runsByDecision(runs: readonly ConsensusRun[]): Map<string, ConsensusRun> {
  const out = new Map<string, ConsensusRun>();
  for (const r of runs) if (r.decisionId) out.set(r.decisionId, r);
  return out;
}

export function DecisionConsensus({ run }: { run: ConsensusRun }) {
  const { t } = useT("consensus");
  const fmt = useFormat();
  const done = run.proposals.filter((p) => p.status === "Done");
  const chosen = run.decision?.chosenIndex ?? null;
  return (
    <section className="cs-dec" aria-label={t("badge")}>
      <div className="cs-dec__head">
        <Badge tone="violet" icon={<Scale size={11} />} title={t("badgeTitle", { count: done.length })}>
          {t("badge")}
        </Badge>
        <span className="rv-muted">{t("models", { count: done.length })}</span>
        {run.decision && <span className="rv-muted">{t("agreement", { value: fmt.percent(run.decision.agreement) })}</span>}
      </div>
      <ul className="cs-dec__alts">
        {orderedProposals(run)
          .filter((p) => p.status === "Done")
          .map((p) => (
            <li key={p.index} className={clsx(p.index === chosen && "cs-dec__alt--chosen")}>
              {p.index === chosen ? <Crown size={12} aria-label={t("chosen")} /> : <span className="cs-dec__dot" aria-hidden="true" />}
              <strong>{p.title || modelLabel(p.answeredBy ?? p.model)}</strong>
              <span className="rv-chip">{modelLabel(p.answeredBy ?? p.model)}</span>
              {p.score !== null && (
                <span className="cs-score" title={t("scoreLabel")}>
                  {t("score", { score: fmt.number(p.score) })}
                </span>
              )}
            </li>
          ))}
      </ul>
    </section>
  );
}
