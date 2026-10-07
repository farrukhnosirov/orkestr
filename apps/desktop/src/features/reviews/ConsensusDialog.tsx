/**
 * Consensus Mode dialog (§8, §68.13), opened from the Manager panel: ask a
 * question, pick 2–5 models (or let routing pick them for a route such as
 * Security), run → side-by-side proposals + the synthesized decision, which
 * is recorded in the Decision Log. Previous runs are listed too.
 */
import "./reviews.css";

import type { ConsensusRun, ModelRef, RunConsensusParams } from "@orkestr/protocol";
import { Badge, Button, IconButton, Spinner } from "@orkestr/ui";
import { ArrowLeft, Plus, Scale, X } from "lucide-react";
import { type FormEvent, useEffect, useMemo, useRef, useState } from "react";

import { useFormat, useT } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { ModelPicker, useProvidersLive } from "../providers";
import { ConsensusRunView } from "./ConsensusRunView";
import { reviewActions, useConsensusRuns } from "./data";
import { modelKey, validateConsensus } from "./model";
import { Overlay } from "../../components/Overlay";

/** Routes offered for automatic participant selection (routing purposes). */
export const CONSENSUS_PURPOSES = ["Consensus", "Architect", "Security", "Backend", "Frontend", "Database"] as const;
type Purpose = (typeof CONSENSUS_PURPOSES)[number];

const MAX_MODELS = 5;

export function ConsensusDialog({ projectId, onClose }: { projectId: string; onClose: () => void }) {
  const { t } = useT(["consensus", "common"]);
  const fmt = useFormat();
  const { providers } = useProvidersLive();
  const { runs, loading, error: listError } = useConsensusRuns(projectId);
  const [question, setQuestion] = useState("");
  const [context, setContext] = useState("");
  const [auto, setAuto] = useState(true);
  const [purpose, setPurpose] = useState<Purpose>("Consensus");
  const [count, setCount] = useState(3);
  const [models, setModels] = useState<(ModelRef | null)[]>([null, null]);
  const [synthesizer, setSynthesizer] = useState<ModelRef | null>(null);
  const [record, setRecord] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [viewId, setViewId] = useState<string | null>(null);
  const questionRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    requestAnimationFrame(() => questionRef.current?.focus());
  }, []);

  // The run just started is shown at once, then replaced by its live version from the list.
  const [started, setStarted] = useState<ConsensusRun | null>(null);
  const viewed = useMemo(
    () => runs.find((r) => r.id === viewId) ?? (started && started.id === viewId ? started : null),
    [runs, viewId, started],
  );
  const picked = models.filter((m): m is ModelRef => m !== null && m.model.trim() !== "");

  const close = () => {
    if (!busy) onClose();
  };

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const problem = validateConsensus(question, picked, auto);
    if (problem) {
      setError(t(`errors.${problem}`));
      return;
    }
    const params: RunConsensusParams = { projectId, question: question.trim(), recordDecision: record };
    if (context.trim()) params.context = context.trim();
    if (auto) {
      params.participants = count;
      if (purpose !== "Consensus") params.purpose = purpose;
    } else {
      const seen = new Set<string>();
      params.models = picked.filter((m) => !seen.has(modelKey(m)) && seen.add(modelKey(m)));
    }
    if (synthesizer && synthesizer.model.trim()) params.synthesizer = synthesizer;
    setBusy(true);
    setError(null);
    try {
      const run = await reviewActions.runConsensus(params);
      setStarted(run);
      setViewId(run.id);
      setQuestion("");
      setContext("");
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Overlay onDismiss={close}>
      <div
        className="dialog cs-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="consensus-title"
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.stopPropagation();
            close();
          }
        }}
      >
        <header className="dialog__header">
          <div className="dialog__icon">
            <Scale size={18} />
          </div>
          <div>
            <h2 id="consensus-title" className="dialog__title">
              {t("dialog.title")}
            </h2>
            <p className="dialog__subtitle">{t("dialog.subtitle")}</p>
          </div>
          <button type="button" className="dialog__close" aria-label={t("common:actions.close")} onClick={close}>
            <X size={16} />
          </button>
        </header>

        {viewed ? (
          <div className="dialog__body cs-body">
            <div>
              <Button size="sm" variant="ghost" leftIcon={<ArrowLeft size={13} />} onClick={() => setViewId(null)}>
                {t("dialog.back")}
              </Button>
            </div>
            <ConsensusRunView run={viewed} />
          </div>
        ) : (
          <div className="dialog__body cs-body cs-body--split">
            <form id="consensus-form" className="cs-form" onSubmit={(e) => void submit(e)}>
              <label className="ok-field">
                <span className="ok-field__label">{t("dialog.question")}</span>
                <textarea
                  ref={questionRef}
                  className="tasks-textarea"
                  rows={3}
                  maxLength={8000}
                  value={question}
                  placeholder={t("dialog.questionPlaceholder")}
                  onChange={(e) => setQuestion(e.target.value)}
                />
              </label>
              <label className="ok-field">
                <span className="ok-field__label">{t("dialog.context")}</span>
                <textarea
                  className="tasks-textarea"
                  rows={2}
                  value={context}
                  placeholder={t("dialog.contextPlaceholder")}
                  onChange={(e) => setContext(e.target.value)}
                />
              </label>
              <fieldset className="ok-field cs-participants">
                <legend className="ok-field__label">{t("dialog.participants")}</legend>
                <div className="cs-radio" role="radiogroup">
                  <label>
                    <input type="radio" name="cs-mode" checked={auto} onChange={() => setAuto(true)} /> {t("dialog.auto")}
                  </label>
                  <label>
                    <input type="radio" name="cs-mode" checked={!auto} onChange={() => setAuto(false)} /> {t("dialog.manual")}
                  </label>
                </div>
                {auto ? (
                  <div className="cs-row">
                    <label className="ok-field">
                      <span className="ok-field__label">{t("dialog.purpose")}</span>
                      <select className="tasks-select" value={purpose} onChange={(e) => setPurpose(e.target.value as Purpose)}>
                        {CONSENSUS_PURPOSES.map((p) => (
                          <option key={p} value={p}>
                            {t(`purposes.${p}`)}
                          </option>
                        ))}
                      </select>
                    </label>
                    <label className="ok-field">
                      <span className="ok-field__label">{t("dialog.count")}</span>
                      <select className="tasks-select" value={count} onChange={(e) => setCount(Number(e.target.value))}>
                        {[2, 3, 4, 5].map((n) => (
                          <option key={n} value={n}>
                            {t("models", { count: n })}
                          </option>
                        ))}
                      </select>
                    </label>
                  </div>
                ) : (
                  <div className="cs-models">
                    {models.map((m, i) => (
                      <div key={i} className="cs-model-row">
                        <ModelPicker
                          label={t("dialog.model", { n: i + 1 })}
                          value={m}
                          onChange={(v) => setModels((cur) => cur.map((x, j) => (j === i ? v : x)))}
                          providers={providers}
                        />
                        {models.length > 2 && (
                          <IconButton
                            size="sm"
                            label={t("dialog.removeModel", { n: i + 1 })}
                            icon={<X size={12} />}
                            onClick={() => setModels((cur) => cur.filter((_, j) => j !== i))}
                          />
                        )}
                      </div>
                    ))}
                    {models.length < MAX_MODELS && (
                      <Button size="sm" variant="ghost" leftIcon={<Plus size={13} />} onClick={() => setModels((cur) => [...cur, null])}>
                        {t("dialog.addModel")}
                      </Button>
                    )}
                  </div>
                )}
              </fieldset>
              <div className="ok-field">
                <ModelPicker label={t("dialog.synthesizer")} value={synthesizer} onChange={setSynthesizer} providers={providers} allowNone />
                <span className="rv-muted">{t("dialog.synthesizerHint")}</span>
              </div>
              <label className="cs-check">
                <input type="checkbox" checked={record} onChange={(e) => setRecord(e.target.checked)} /> {t("dialog.record")}
              </label>
              {error && (
                <p className="tasks-error" role="alert">
                  {error}
                </p>
              )}
            </form>
            <aside className="cs-history" aria-label={t("dialog.history")}>
              <h4 className="cs-subtitle">{t("dialog.history")}</h4>
              {listError ? (
                <p className="tasks-error">{listError}</p>
              ) : loading && runs.length === 0 ? (
                <p className="rv-muted rv-progress">
                  <Spinner size={13} /> {t("loading")}
                </p>
              ) : runs.length === 0 ? (
                <p className="rv-muted">{t("empty")}</p>
              ) : (
                <ul className="cs-history__list">
                  {runs.map((r) => (
                    <li key={r.id}>
                      <button type="button" className="cs-history__item" onClick={() => setViewId(r.id)}>
                        <span className="cs-history__q">{r.question}</span>
                        <span className="rv-meta">
                          <Badge tone={r.status === "Finished" ? "success" : r.status === "Failed" ? "danger" : "accent"}>
                            {t(`status.${r.status}`)}
                          </Badge>
                          <span>{t("models", { count: r.proposals.length })}</span>
                          <time dateTime={r.createdAt}>{fmt.relative(r.createdAt)}</time>
                        </span>
                      </button>
                    </li>
                  ))}
                </ul>
              )}
            </aside>
          </div>
        )}

        <footer className="dialog__footer">
          {viewed ? (
            <Button variant="primary" leftIcon={<Plus size={14} />} onClick={() => setViewId(null)}>
              {t("dialog.newRun")}
            </Button>
          ) : (
            <>
              <Button variant="ghost" onClick={close} disabled={busy}>
                {t("common:actions.cancel")}
              </Button>
              <Button variant="primary" type="submit" form="consensus-form" loading={busy}>
                {t("dialog.run")}
              </Button>
            </>
          )}
        </footer>
      </div>
    </Overlay>
  );
}
