/**
 * Bug Detective settings (`detective.settings` / `detective.setSettings`,
 * ADR-0042): auto-open on alert, default budget per incident and the number
 * of fix attempts. Saving is local-only on the core (a remote device is
 * refused), so errors are shown inline.
 */
import { Button, Spinner } from "@orkestr/ui";
import { useEffect, useState } from "react";

import { getClient } from "../../core/client";
import { useT } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { draftToSettings, type SettingsDraft, settingsToDraft } from "./model";

const ATTEMPTS = [1, 2, 3, 4, 5];

export function DetectiveSettingsForm({ onClose }: { onClose: () => void }) {
  const { t } = useT("detective");
  const [draft, setDraft] = useState<SettingsDraft | null>(null);
  const [invalid, setInvalid] = useState<"budget" | "attempts" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let live = true;
    getClient()
      .call("detective.settings", {})
      .then((s) => live && setDraft(settingsToDraft(s)))
      .catch((e: unknown) => live && setError(describeError(e)));
    return () => {
      live = false;
    };
  }, []);

  const set = (patch: Partial<SettingsDraft>) => {
    setSaved(false);
    setDraft((d) => (d ? { ...d, ...patch } : d));
  };

  const save = async () => {
    if (!draft) return;
    const res = draftToSettings(draft);
    if ("error" in res) {
      setInvalid(res.error);
      return;
    }
    setInvalid(null);
    setError(null);
    setSaving(true);
    try {
      setDraft(settingsToDraft(await getClient().call("detective.setSettings", res.settings)));
      setSaved(true);
    } catch (e) {
      setError(describeError(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <form
      className="bd-report bd-settings"
      noValidate
      aria-label={t("settings.title")}
      data-testid="detective-settings"
      onSubmit={(e) => {
        e.preventDefault();
        void save();
      }}
    >
      <strong>{t("settings.title")}</strong>
      {!draft ? (
        error ? (
          <div className="bd-error" role="alert">
            {error}
          </div>
        ) : (
          <Spinner label={t("loading")} />
        )
      ) : (
        <>
          <label className="bd-check">
            <input
              type="checkbox"
              checked={draft.autoOpenOnAlert}
              onChange={(e) => set({ autoOpenOnAlert: e.target.checked })}
              data-testid="settings-auto-open"
            />{" "}
            {t("settings.autoOpen")}
          </label>
          <p className="bd-muted bd-small">{t("settings.autoOpenHint")}</p>
          <label className="bd-inline">
            {t("settings.budget")}
            <input
              type="number"
              min="0.01"
              max="1000"
              step="0.01"
              value={draft.defaultBudgetUsd}
              aria-invalid={invalid === "budget"}
              onChange={(e) => set({ defaultBudgetUsd: e.target.value })}
              data-testid="settings-budget"
            />
          </label>
          <p className={invalid === "budget" ? "bd-error" : "bd-muted bd-small"}>
            {invalid === "budget" ? t("settings.error.budget") : t("settings.budgetHint")}
          </p>
          <label className="bd-inline">
            {t("settings.attempts")}
            <select
              value={draft.maxFixAttempts}
              onChange={(e) => set({ maxFixAttempts: Number(e.target.value) })}
              data-testid="settings-attempts"
            >
              {ATTEMPTS.map((n) => (
                <option key={n} value={n}>
                  {n}
                </option>
              ))}
            </select>
          </label>
          {invalid === "attempts" && <p className="bd-error">{t("settings.error.attempts")}</p>}
          {error && (
            <div className="bd-error" role="alert">
              {error}
            </div>
          )}
          {saved && (
            <p className="bd-muted" role="status">
              {t("settings.saved")}
            </p>
          )}
        </>
      )}
      <div className="bd-row">
        <Button type="submit" size="sm" variant="primary" loading={saving} disabled={!draft} data-testid="settings-save">
          {t("settings.save")}
        </Button>
        <Button type="button" size="sm" variant="ghost" onClick={onClose}>
          {t("settings.close")}
        </Button>
      </div>
    </form>
  );
}
