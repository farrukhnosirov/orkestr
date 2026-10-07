/**
 * Limits editor of the parallel scheduler (`scheduler.setLimits`): global /
 * per-project caps, this project's cap, per-provider agent slots, host
 * resource thresholds, aging, fairness, rate-limit back-off and the
 * auto-resume policy after a restart.
 */
import type { AutoResumePolicy, SchedulerStatus } from "@orkestr/protocol";
import { Button } from "@orkestr/ui";
import { Gauge, X } from "lucide-react";
import { type FormEvent, useEffect, useState } from "react";

import { useT } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { schedulerActions } from "./data";
import { fromForm, type LimitsForm, shortId, toForm } from "./model";
import { Overlay } from "../../components/Overlay";

const POLICIES: readonly AutoResumePolicy[] = ["off", "followApproval", "always"];
const POLICY_KEY = { off: "limits.autoResumeOff", followApproval: "limits.autoResumeFollowApproval", always: "limits.autoResumeAlways" } as const;

type NumField = Exclude<keyof LimitsForm, "fairShare" | "mergeRiskChecks" | "autoResume" | "providerCaps">;

export function LimitsDialog({
  status,
  projectId,
  onClose,
  onSaved,
}: {
  status: SchedulerStatus;
  projectId: string | null;
  onClose: () => void;
  onSaved: () => void;
}) {
  const { t } = useT(["scheduler", "common"]);
  const [form, setForm] = useState<LimitsForm>(() => toForm(status.settings, projectId));
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    setForm(toForm(status.settings, projectId));
    // eslint-disable-next-line react-hooks/exhaustive-deps -- reset only when the dialog (re)opens for a project
  }, [projectId]);

  const set = (patch: Partial<LimitsForm>) => setForm((f) => ({ ...f, ...patch }));
  const digits = (v: string) => v.replace(/[^0-9]/g, "");

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const settings = fromForm(status.settings, form, projectId);
    if (!settings) {
      setError(t("limits.invalid"));
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await schedulerActions.setLimits(settings);
      onSaved();
      onClose();
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  };

  const dismiss = () => {
    if (!busy) onClose();
  };

  const num = (field: NumField, label: string, hint?: string) => (
    <label className="ok-field sched-field">
      <span className="ok-field__label">{label}</span>
      <input
        className="sched-input"
        inputMode="numeric"
        value={form[field]}
        onChange={(e) => set({ [field]: digits(e.target.value) } as Partial<LimitsForm>)}
      />
      {hint && <span className="sched-hint">{hint}</span>}
    </label>
  );

  return (
    <Overlay onDismiss={dismiss}>
      <form
        className="dialog sched-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="sched-limits-title"
        onSubmit={(e) => void submit(e)}
        onKeyDown={(e) => e.key === "Escape" && dismiss()}
      >
        <header className="dialog__header">
          <div className="dialog__icon">
            <Gauge size={18} />
          </div>
          <div>
            <h2 id="sched-limits-title" className="dialog__title">
              {t("limits.title")}
            </h2>
            <p className="dialog__subtitle">{t("limits.subtitle")}</p>
          </div>
          <button type="button" className="dialog__close" aria-label={t("common:actions.close")} onClick={dismiss}>
            <X size={16} />
          </button>
        </header>
        <div className="dialog__body sched-dialog__body">
          <div className="sched-grid">
            {num("maxParallelGlobal", t("limits.global"))}
            {num("maxParallelPerProject", t("limits.perProject"))}
            {projectId && num("projectCap", t("limits.thisProject"), t("limits.thisProjectHint"))}
            {num("maxCpuPercent", t("limits.cpu"))}
            {num("minFreeRamMb", t("limits.ram"))}
            {num("minFreeDiskMb", t("limits.disk"))}
            {num("maxGpuPercent", t("limits.gpu"), t("limits.gpuHint"))}
            {num("agingMinutes", t("limits.aging"), t("limits.agingHint"))}
            {num("rateLimitBackoffSecs", t("limits.backoff"))}
            {num("maxAutoResumeAttempts", t("limits.resumeAttempts"))}
          </div>
          <label className="sched-check-row">
            <input type="checkbox" checked={form.fairShare} onChange={(e) => set({ fairShare: e.target.checked })} />
            {t("limits.fairShare")}
          </label>
          <label className="sched-check-row">
            <input
              type="checkbox"
              checked={form.mergeRiskChecks}
              onChange={(e) => set({ mergeRiskChecks: e.target.checked })}
            />
            {t("limits.mergeRisk")}
          </label>
          <label className="ok-field sched-field">
            <span className="ok-field__label">{t("limits.autoResume")}</span>
            <select
              className="sched-select"
              value={form.autoResume}
              onChange={(e) => set({ autoResume: e.target.value as AutoResumePolicy })}
            >
              {POLICIES.map((p) => (
                <option key={p} value={p}>
                  {t(POLICY_KEY[p])}
                </option>
              ))}
            </select>
          </label>
          <fieldset className="ok-field sched-fieldset">
            <legend className="ok-field__label">{t("limits.providerCaps")}</legend>
            {status.providers.length === 0 ? (
              <p className="sched-muted">{t("limits.noProviders")}</p>
            ) : (
              <div className="sched-grid">
                {status.providers.map((p) => (
                  <label key={p.providerId} className="ok-field sched-field">
                    <span className="ok-field__label">{p.name ?? shortId(p.providerId)}</span>
                    <input
                      className="sched-input"
                      inputMode="numeric"
                      placeholder={t("limits.providerCapHint")}
                      value={form.providerCaps[p.providerId] ?? ""}
                      onChange={(e) =>
                        set({ providerCaps: { ...form.providerCaps, [p.providerId]: digits(e.target.value) } })
                      }
                    />
                  </label>
                ))}
              </div>
            )}
          </fieldset>
          {error && (
            <p className="sched-error" role="alert">
              {error}
            </p>
          )}
        </div>
        <footer className="dialog__footer">
          <Button variant="ghost" onClick={dismiss} disabled={busy}>
            {t("common:actions.cancel")}
          </Button>
          <Button variant="primary" type="submit" loading={busy}>
            {t("limits.save")}
          </Button>
        </footer>
      </form>
    </Overlay>
  );
}
