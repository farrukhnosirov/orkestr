import type { Decision, DecisionAlternative, DecisionStatus } from "@orkestr/protocol";
import { topic } from "@orkestr/protocol";
import { Badge, Button, EmptyState, Input, Spinner } from "@orkestr/ui";
import { clsx } from "clsx";
import { Plus, Scale, X } from "lucide-react";
import { type FormEvent, useState } from "react";

import { useFormat, useT } from "../../i18n";
import { errorMessage, requireClient, useRpcQuery } from "../../lib/rpcQuery";
import { DECISION_STATUS_TONE } from "./kinds";
import { Markdown } from "./markdown";

const STATUSES: DecisionStatus[] = ["Proposed", "Accepted", "Rejected", "Superseded"];

/** Parses "Title: why rejected" lines into alternatives. */
export function parseAlternatives(text: string): DecisionAlternative[] {
  return text
    .split("\n")
    .map((l) => l.trim().replace(/^[-*]\s*/, ""))
    .filter(Boolean)
    .map((line) => {
      const idx = line.indexOf(":");
      const title = (idx > 0 ? line.slice(0, idx) : line).trim();
      const reason = idx > 0 ? line.slice(idx + 1).trim() : "";
      return { title, summary: "", proposedBy: null, rejectedReason: reason || null };
    });
}

function DecisionDialog({ projectId, onClose, onSaved }: { projectId: string; onClose: () => void; onSaved: () => void }) {
  const { t } = useT(["memory", "common"]);
  const [title, setTitle] = useState("");
  const [decision, setDecision] = useState("");
  const [context, setContext] = useState("");
  const [alternatives, setAlternatives] = useState("");
  const [reasoning, setReasoning] = useState("");
  const [consequences, setConsequences] = useState("");
  const [files, setFiles] = useState("");
  const [status, setStatus] = useState<DecisionStatus>("Accepted");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const close = () => {
    if (!busy) onClose();
  };
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!title.trim() || !decision.trim()) {
      setError(t("decisions.dialog.required"));
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await requireClient().call("decisions.create", {
        projectId,
        title: title.trim(),
        decision: decision.trim(),
        context,
        alternatives: parseAlternatives(alternatives),
        reasoningSummary: reasoning,
        consequences,
        status,
        files: files
          .split(/[,\n]/)
          .map((f) => f.trim())
          .filter(Boolean),
        decidedBy: { type: "System" },
      });
      onSaved();
    } catch (err) {
      setError(errorMessage(err));
      setBusy(false);
    }
  };

  const area = (id: string, label: string, value: string, set: (v: string) => void, placeholder: string, rows = 3) => (
    <div className="ok-field">
      <label className="ok-field__label" htmlFor={id}>
        {label}
      </label>
      <textarea id={id} className="mem-textarea" rows={rows} value={value} placeholder={placeholder} onChange={(e) => set(e.target.value)} />
    </div>
  );

  return (
    <div className="overlay" role="presentation" onMouseDown={(e) => e.target === e.currentTarget && close()}>
      <form
        className="dialog mem-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="decision-title"
        onSubmit={(e) => void submit(e)}
        onKeyDown={(e) => e.key === "Escape" && close()}
      >
        <header className="dialog__header">
          <div className="dialog__icon">
            <Scale size={18} />
          </div>
          <div>
            <h2 id="decision-title" className="dialog__title">
              {t("decisions.dialog.title")}
            </h2>
            <p className="dialog__subtitle">{t("decisions.dialog.subtitle")}</p>
          </div>
          <button type="button" className="dialog__close" aria-label={t("common:actions.close")} onClick={close}>
            <X size={16} />
          </button>
        </header>
        <div className="dialog__body mem-dialog__scroll">
          <div className="mem-form-row">
            <Input className="mem-field--grow" label={t("decisions.dialog.titleField")} value={title} maxLength={200} onChange={(e) => setTitle(e.target.value)} autoFocus />
            <label className="ok-field mem-field--kind">
              <span className="ok-field__label">{t("decisions.dialog.status")}</span>
              <select className="mem-select" value={status} onChange={(e) => setStatus(e.target.value as DecisionStatus)}>
                {STATUSES.map((s) => (
                  <option key={s} value={s}>
                    {t(`decisions.status.${s}`)}
                  </option>
                ))}
              </select>
            </label>
          </div>
          {area("dec-decision", t("decisions.dialog.decision"), decision, setDecision, t("decisions.dialog.decisionPlaceholder"), 2)}
          {area("dec-context", t("decisions.dialog.context"), context, setContext, t("decisions.dialog.contextPlaceholder"))}
          {area("dec-alts", t("decisions.dialog.alternatives"), alternatives, setAlternatives, t("decisions.dialog.alternativesPlaceholder"))}
          {area("dec-reason", t("decisions.dialog.reasoning"), reasoning, setReasoning, t("decisions.dialog.reasoningPlaceholder"))}
          {area("dec-cons", t("decisions.dialog.consequences"), consequences, setConsequences, t("decisions.dialog.consequencesPlaceholder"), 2)}
          <Input label={t("decisions.dialog.files")} placeholder={t("decisions.dialog.filesPlaceholder")} value={files} onChange={(e) => setFiles(e.target.value)} />
          {error && (
            <div className="ok-field__error" role="alert">
              {error}
            </div>
          )}
        </div>
        <footer className="dialog__footer">
          <Button variant="ghost" onClick={close} disabled={busy}>
            {t("common:actions.cancel")}
          </Button>
          <Button variant="primary" type="submit" loading={busy}>
            {t("decisions.dialog.submit")}
          </Button>
        </footer>
      </form>
    </div>
  );
}

/** Decision log timeline (§21). */
export function DecisionLog({ projectId }: { projectId: string }) {
  const { t } = useT(["memory", "common"]);
  const fmt = useFormat();
  const [creating, setCreating] = useState(false);
  const [statusFilter, setStatusFilter] = useState<DecisionStatus | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const { data, error, loading, reload } = useRpcQuery<Decision[]>(
    `decisions:${projectId}:${statusFilter ?? ""}`,
    async (client) =>
      (await client.call("decisions.list", { projectId, ...(statusFilter ? { status: statusFilter } : {}) })).decisions,
    { topics: [topic.project(projectId)], reloadOn: ["DecisionRecorded"] },
  );
  const decisions = data ?? [];

  const setStatus = async (d: Decision, status: DecisionStatus) => {
    setActionError(null);
    try {
      await requireClient().call("decisions.update", { id: d.id, patch: { status } });
      reload();
    } catch (e) {
      setActionError(errorMessage(e));
    }
  };

  return (
    <div className="mem-view">
      <div className="mem-toolbar">
        <div className="mem-chips mem-chips--inline" role="group" aria-label={t("decisions.filterByStatus")}>
          <button type="button" className={clsx("mem-chip", statusFilter === null && "mem-chip--active")} aria-pressed={statusFilter === null} onClick={() => setStatusFilter(null)}>
            {t("decisions.all")}
          </button>
          {STATUSES.map((s) => (
            <button
              key={s}
              type="button"
              className={clsx("mem-chip", statusFilter === s && "mem-chip--active")}
              aria-pressed={statusFilter === s}
              onClick={() => setStatusFilter(statusFilter === s ? null : s)}
            >
              {t(`decisions.status.${s}`)}
            </button>
          ))}
        </div>
        <div className="mem-toolbar__spacer" />
        {loading && <Spinner size={14} />}
        <Button variant="primary" size="sm" leftIcon={<Plus size={14} />} onClick={() => setCreating(true)}>
          {t("decisions.record")}
        </Button>
      </div>
      {(error || actionError) && (
        <div className="mem-error" role="alert">
          {actionError ?? error}
          {error && (
            <Button size="sm" variant="ghost" onClick={reload}>
              {t("common:actions.retry")}
            </Button>
          )}
        </div>
      )}
      <div className="mem-scroll">
        {loading && data === null ? (
          <div className="mem-loading">
            <Spinner size={18} /> {t("decisions.loading")}
          </div>
        ) : decisions.length === 0 && !error ? (
          <EmptyState
            icon={<Scale size={26} strokeWidth={1.6} />}
            title={statusFilter ? t(`decisions.emptyByStatus.${statusFilter}`) : t("decisions.emptyTitle")}
            description={t("decisions.emptyDescription")}
            action={
              <Button variant="primary" leftIcon={<Plus size={14} />} onClick={() => setCreating(true)}>
                {t("decisions.record")}
              </Button>
            }
          />
        ) : (
          <ol className="dec-timeline" aria-label={t("decisions.timeline")}>
            {decisions.map((d) => (
              <li key={d.id} className={clsx("dec-entry", `dec-entry--${d.status.toLowerCase()}`)}>
                <div className="dec-entry__dot" aria-hidden="true" />
                <div className="dec-entry__card">
                  <div className="dec-entry__head">
                    <span className="dec-entry__num">{t("decisions.number", { number: d.number })}</span>
                    <h3 className="dec-entry__title">{d.title}</h3>
                    <Badge tone={DECISION_STATUS_TONE[d.status]}>{t(`decisions.status.${d.status}`)}</Badge>
                    <span className="dec-entry__time" title={fmt.date(d.createdAt)}>
                      {fmt.relative(d.createdAt)}
                    </span>
                  </div>
                  <Markdown source={d.decision} className="md dec-entry__decision" />
                  {d.context && (
                    <section>
                      <h4>{t("decisions.context")}</h4>
                      <Markdown source={d.context} />
                    </section>
                  )}
                  {d.alternatives.length > 0 && (
                    <section>
                      <h4>{t("decisions.alternatives")}</h4>
                      <ul className="dec-alts">
                        {d.alternatives.map((a, i) => (
                          <li key={i}>
                            <strong>{a.title}</strong>
                            {a.summary && ` — ${a.summary}`}
                            {a.rejectedReason && <span className="mem-muted"> {t("decisions.notChosen", { reason: a.rejectedReason })}</span>}
                          </li>
                        ))}
                      </ul>
                    </section>
                  )}
                  {d.reasoningSummary && (
                    <section>
                      <h4>{t("decisions.reasoning")}</h4>
                      <Markdown source={d.reasoningSummary} />
                    </section>
                  )}
                  {d.consequences && (
                    <section>
                      <h4>{t("decisions.consequences")}</h4>
                      <Markdown source={d.consequences} />
                    </section>
                  )}
                  {(d.files.length > 0 || d.taskIds.length > 0) && (
                    <div className="mem-card__meta">
                      {d.files.map((f) => (
                        <code key={f} className="mem-tag">
                          {f}
                        </code>
                      ))}
                      {d.taskIds.length > 0 && <span>{t("decisions.relatedTasks", { count: d.taskIds.length })}</span>}
                    </div>
                  )}
                  {d.status === "Proposed" && (
                    <div className="dec-entry__actions">
                      <Button size="sm" variant="secondary" onClick={() => void setStatus(d, "Accepted")}>
                        {t("decisions.accept")}
                      </Button>
                      <Button size="sm" variant="ghost" onClick={() => void setStatus(d, "Rejected")}>
                        {t("decisions.reject")}
                      </Button>
                    </div>
                  )}
                </div>
              </li>
            ))}
          </ol>
        )}
      </div>
      {creating && (
        <DecisionDialog
          projectId={projectId}
          onClose={() => setCreating(false)}
          onSaved={() => {
            setCreating(false);
            reload();
          }}
        />
      )}
    </div>
  );
}
