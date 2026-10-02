import { clsx } from "clsx";
import { useState } from "react";

import { useSettingsStore } from "../features/settings/settingsStore";
import { type LanguagePreference, languagePreference, SUPPORTED_LANGUAGES, systemLanguage, toPreference, useT } from "../i18n";
import { switchLanguage } from "../lib/language";

/**
 * Interface language picker (Settings → General, onboarding step 1).
 * Switches live and saves `AppSettings.language` (`ru` | `en` | `system`).
 * Language names are always shown in their own language.
 */
export function LanguageSelect({ className, id }: { className?: string; id?: string }) {
  const { t } = useT("common");
  const stored = useSettingsStore((s) => s.settings?.language);
  const [local, setLocal] = useState<LanguagePreference>(() => toPreference(stored ?? languagePreference()));
  const value = stored !== undefined ? toPreference(stored) : local;
  const systemName = t(`language.names.${systemLanguage()}`);

  return (
    <select
      id={id}
      className={clsx("set-select", className)}
      aria-label={t("language.label")}
      value={value}
      onChange={(e) => {
        const next = toPreference(e.target.value);
        setLocal(next);
        void switchLanguage(next);
      }}
    >
      <option value="system">{t("language.system", { language: systemName })}</option>
      {SUPPORTED_LANGUAGES.map((lng) => (
        <option key={lng} value={lng} lang={lng}>
          {t(`language.names.${lng}`)}
        </option>
      ))}
    </select>
  );
}
