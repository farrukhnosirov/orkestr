import type { BudgetPeriod, BudgetRule, BudgetScope, Project } from "@orkestr/protocol";
import { Button, Input } from "@orkestr/ui";
import { Coins, X } from "lucide-react";
import { type FormEvent, useState } from "react";

import { t as translate, useT } from "../../i18n";
import { errorMessage, requireClient } from "../../lib/rpcQuery";
import { periodLabel } from "./format";

type ScopeKind = BudgetScope["scope"];

const SCOPE_KINDS: ScopeKind[] = ["Global", "Project", "Provider", "Agent", "Task"];

const PERIODS: BudgetPeriod[] = ["Monthly", "Daily", "PerTask", "Total"];

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

function scopeId(scope: BudgetScope | undefined): string {
  if (!scope) return "";
  switch (scope.scope) {
    case "Project":
      return scope.projectId;
    case "Provider":
      return scope.providerId;
    case "Agent":
      return scope.agentId;
    case "Task":
      return scope.taskId;
    default:
      return "";
  }
}

/** Builds a scope from the form, or returns a (localized) error message. */
export function buildScope(kind: ScopeKind, id: string): BudgetScope | string {
  const v = id.trim();
  if (kind === "Global") return { scope: "Global" };
  if (!UUID.test(v)) return translate(`costs:editor.invalidId.${kind}`);
  switch (kind) {
    case "Project":
      return { scope: "Project", projectId: v };
    case "Provider":
      return { scope: "Provider", providerId: v };
    case "Agent":
      return { scope: "Agent", agentId: v };
    default:
      return { scope: "Task", taskId: v };
  }
}

export interface BudgetEditorProps {
  budget: BudgetRule | null;
  projects: readonly Project[];
  defaultProjectId: string | null;
  onClose: () => void;
  onSaved: () => void;
}

/** Create / edit a budget (`budgets.set`). */
export function BudgetEditor({ budget, projects, defaultProjectId, onClose, onSaved }: BudgetEditorProps) {
  const { t } = useT(["costs", "common"]);
  const [kind, setKind] = useState<ScopeKind>(budget?.scope.scope ?? (defaultProjectId ? "Project" : "Global"));
  const [id, setId] = useState(budget ? scopeId(budget.scope) : (defaultProjectId ?? ""));
  const [period, setPeriod] = useState<BudgetPeriod>(budget?.period ?? "Monthly");
  const [name, setName] = useState(budget?.name ?? "");
  const [limit, setLimit] = useState(budget?.limitUsd != null ? String(budget.limitUsd) : "");
  const [unrestricted, setUnrestricted] = useState(budget?.unrestricted ?? false);
  const [warnAt, setWarnAt] = useState(String(Math.round((budget?.warnAt ?? 0.8) * 100)));
  const [enabled, setEnabled] = useState(budget?.enabled ?? true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const close = () => {
    if (!busy) onClose();
  };

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const scope = buildScope(kind, id);
    if (typeof scope === "string") {
      setError(scope);
      return;
    }
    const limitUsd = Number(limit);
    if (!unrestricted && (!limit.trim() || !Number.isFinite(limitUsd) || limitUsd < 0)) {
      setError(t("editor.limitRequired"));
      return;
    }
    const warn = Number(warnAt) / 100;
    if (!(warn > 0 && warn < 1)) {
      setError(t("editor.warnRange"));
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await requireClient().call("budgets.set", {
        ...(budget ? { id: budget.id } : {}),
        ...(name.trim() ? { name: name.trim() } : {}),
        scope,
        period,
        ...(unrestricted ? { unrestricted: true } : { limitUsd }),
        warnAt: warn,
        enabled,
      });
      onSaved();
    } catch (err) {
      setError(errorMessage(err));
      setBusy(false);
    }
  };

  return (
    <div className="overlay" role="presentation" onMouseDown={(e) => e.target === e.currentTarget && close()}>
      <form
        className="dialog cost-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="budget-editor-title"
        onSubmit={(e) => void submit(e)}
        onKeyDown={(e) => e.key === "Escape" && close()}
      >
        <header className="dialog__header">
          <div className="dialog__icon">
            <Coins size={18} />
          </div>
          <div>
            <h2 id="budget-editor-title" className="dialog__title">
              {budget ? t("editor.titleEdit") : t("editor.titleNew")}
            </h2>
            <p className="dialog__subtitle">{t("editor.subtitle")}</p>
          </div>
          <button type="button" className="dialog__close" aria-label={t("common:actions.close")} onClick={close}>
            <X size={16} />
          </button>
        </header>
        <div className="dialog__body">
          <div className="cost-form-row">
            <label className="ok-field cost-field">
              <span className="ok-field__label">{t("editor.scope")}</span>
              <select
                className="cost-select"
                value={kind}
                onChange={(e) => {
                  const k = e.target.value as ScopeKind;
                  setKind(k);
                  setId(k === "Project" ? (defaultProjectId ?? projects[0]?.id ?? "") : "");
                }}
              >
                {SCOPE_KINDS.map((s) => (
                  <option key={s} value={s}>
                    {t(`editor.scopeKind.${s}`)}
                  </option>
                ))}
              </select>
            </label>
            {kind === "Project" ? (
              <label className="ok-field cost-field cost-field--grow">
                <span className="ok-field__label">{t("editor.project")}</span>
                <select className="cost-select" value={id} onChange={(e) => setId(e.target.value)}>
                  {projects.map((p) => (
                    <option key={p.id} value={p.id}>
                      {p.name}
                    </option>
                  ))}
                </select>
              </label>
            ) : kind !== "Global" ? (
              <Input className="cost-field--grow" label={t(`editor.idLabel.${kind}`)} placeholder={t("editor.idPlaceholder")} value={id} onChange={(e) => setId(e.target.value)} />
            ) : null}
          </div>
          <div className="cost-form-row">
            <label className="ok-field cost-field">
              <span className="ok-field__label">{t("editor.period")}</span>
              <select className="cost-select" value={period} onChange={(e) => setPeriod(e.target.value as BudgetPeriod)}>
                {PERIODS.map((p) => (
                  <option key={p} value={p}>
                    {periodLabel(p)}
                  </option>
                ))}
              </select>
            </label>
            <Input
              className="cost-field--grow"
              label={t("editor.limit")}
              type="number"
              min={0}
              step="0.01"
              inputMode="decimal"
              value={limit}
              disabled={unrestricted}
              onChange={(e) => setLimit(e.target.value)}
              placeholder={unrestricted ? t("editor.unrestrictedPlaceholder") : "100"}
            />
            <Input className="cost-field" label={t("editor.warnAt")} type="number" min={1} max={99} value={warnAt} onChange={(e) => setWarnAt(e.target.value)} />
          </div>
          <Input label={t("editor.name")} placeholder={t("editor.namePlaceholder")} value={name} maxLength={120} onChange={(e) => setName(e.target.value)} />
          <div className="cost-checks">
            <label className="cost-check">
              <input type="checkbox" checked={unrestricted} onChange={(e) => setUnrestricted(e.target.checked)} />
              {t("editor.unrestricted")}
            </label>
            <label className="cost-check">
              <input type="checkbox" checked={enabled} onChange={(e) => setEnabled(e.target.checked)} />
              {t("editor.enabled")}
            </label>
          </div>
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
            {budget ? t("common:actions.save") : t("editor.create")}
          </Button>
        </footer>
      </form>
    </div>
  );
}
