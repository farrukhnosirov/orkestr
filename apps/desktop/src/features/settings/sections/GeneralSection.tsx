import type { AppSettings, PerformanceMode } from "@orkestr/protocol";
import { Button, EmptyState, Input, Spinner } from "@orkestr/ui";
import { AlertTriangle, Palette, Wand2 } from "lucide-react";
import { useEffect, useState } from "react";

import { LanguageSelect } from "../../../components/LanguageSelect";
import { t, useT } from "../../../i18n";
import { useOnboardingStore } from "../../onboarding/onboardingStore";
import { Segmented, SettingGroup, SettingRow, Switch } from "../controls";
import type { SettingsSectionProps } from "../sections";
import { useSettingsStore } from "../settingsStore";
import { useThemeStore } from "../../theme/themeStore";

/** Retention presets in days (0 = forever). */
export const RETENTION_PRESETS: readonly string[] = ["30", "90", "180", "365", "0"];

const PERFORMANCE_MODES: readonly PerformanceMode[] = ["UltraVisual", "Balanced", "Performance", "BatterySaver"];

/** Localized label for a retention value in days. */
export function retentionLabel(days: string): string {
  const n = Number(days);
  if (n === 0) return t("settings:general.retentionForever");
  if (n === 365) return t("settings:general.retentionYear");
  return t("common:count.days", { count: n });
}

/** Settings → General. */
export function GeneralSection(_props: SettingsSectionProps) {
  const { settings, loading, error, saving, autostartSupported, update, load } = useSettingsStore();
  const setSection = useSettingsStore((s) => s.setSection);
  const activeThemeName = useThemeStore((s) => s.config?.theme.name ?? s.fallback.name);
  const openWizard = useOnboardingStore((s) => s.openWizard);
  const closeSettings = useSettingsStore((s) => s.closeSettings);
  const [name, setName] = useState("");
  useT(["settings", "common", "theme"]); // re-render on language change

  const profileName = settings?.profileName;
  useEffect(() => {
    if (profileName !== undefined) setName(profileName);
  }, [profileName]);

  if (!settings) {
    if (loading) return <div className="set-loading"><Spinner size={20} /> {t("settings:loadingSettings")}</div>;
    return (
      <EmptyState
        icon={<AlertTriangle size={22} />}
        title={t("settings:unavailable.title")}
        description={error ?? t("settings:unavailable.noData")}
        action={<Button onClick={() => void load()}>{t("common:actions.retry")}</Button>}
      />
    );
  }

  const set = <K extends keyof AppSettings>(key: K, value: AppSettings[K]) => void update({ [key]: value });
  const retention = String(settings.eventRetentionDays);
  const retentionOptions = (RETENTION_PRESETS.includes(retention) ? RETENTION_PRESETS : [...RETENTION_PRESETS, retention]).map(
    (value) => ({ value, label: retentionLabel(value) }),
  );
  const performanceOptions = PERFORMANCE_MODES.map((value) => ({
    value,
    label: t(`settings:general.performance.${value}.label`),
    title: t(`settings:general.performance.${value}.title`),
  }));

  return (
    <div className="set-section">
      {error && (
        <div className="set-alert" role="alert">
          <AlertTriangle size={14} /> {error}
        </div>
      )}
      <SettingGroup title={t("settings:general.profile")}>
        <SettingRow title={t("settings:general.displayName")} description={t("settings:general.displayNameDesc")}>
          <Input
            aria-label={t("settings:general.displayName")}
            value={name}
            maxLength={80}
            placeholder={t("settings:general.displayNamePlaceholder")}
            onChange={(e) => setName(e.target.value)}
            onBlur={() => name.trim() !== settings.profileName && set("profileName", name.trim())}
            onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()}
          />
        </SettingRow>
        <SettingRow title={t("common:language.label")} description={t("common:language.description")}>
          <LanguageSelect />
        </SettingRow>
      </SettingGroup>

      <SettingGroup title={t("settings:general.background")}>
        <SettingRow
          title={t("settings:general.closeToTray")}
          description={t("settings:general.closeToTrayDesc")}
        >
          <Switch
            label={t("settings:general.closeToTrayLabel")}
            checked={settings.closeToTray}
            busy={saving === "closeToTray"}
            onChange={(v) => set("closeToTray", v)}
          />
        </SettingRow>
        <SettingRow
          title={t("settings:general.autostart")}
          description={
            autostartSupported
              ? t("settings:general.autostartDesc")
              : t("settings:general.autostartUnsupported")
          }
        >
          <Switch
            label={t("settings:general.autostartLabel")}
            checked={settings.autostart}
            disabled={!autostartSupported}
            busy={saving === "autostart"}
            onChange={(v) => set("autostart", v)}
          />
        </SettingRow>
      </SettingGroup>

      <SettingGroup title={t("settings:general.appearance")}>
        <SettingRow title={t("settings:general.theme")} description={activeThemeName}>
          <Button size="sm" leftIcon={<Palette size={14} />} onClick={() => setSection("appearance")}>
            {t("theme:commands.openAppearance")}
          </Button>
        </SettingRow>
        <SettingRow title={t("settings:general.performanceMode")} description={t("settings:general.performanceModeDesc")} stacked>
          <Segmented
            label={t("settings:general.performanceMode")}
            value={settings.performanceMode}
            options={performanceOptions}
            onChange={(v) => set("performanceMode", v)}
          />
        </SettingRow>
      </SettingGroup>

      <SettingGroup title={t("settings:general.notifications")}>
        <SettingRow title={t("settings:general.dnd")} description={t("settings:general.dndDesc")}>
          <Switch
            label={t("settings:general.dnd")}
            checked={settings.doNotDisturb}
            busy={saving === "doNotDisturb"}
            onChange={(v) => set("doNotDisturb", v)}
          />
        </SettingRow>
      </SettingGroup>

      <SettingGroup title={t("settings:general.data")}>
        <SettingRow
          title={t("settings:general.retention")}
          description={t("settings:general.retentionDesc")}
          stacked
        >
          <Segmented
            label={t("settings:general.retentionLabel")}
            value={retention}
            options={retentionOptions}
            onChange={(v) => set("eventRetentionDays", Number(v))}
          />
        </SettingRow>
        <SettingRow title={t("settings:general.telemetry")} description={t("settings:general.telemetryDesc")}>
          <Switch label={t("settings:general.telemetryLabel")} checked={false} disabled onChange={() => {}} />
        </SettingRow>
      </SettingGroup>

      <SettingGroup title={t("settings:general.setup")}>
        <SettingRow title={t("settings:general.wizard")} description={t("settings:general.wizardDesc")}>
          <Button
            leftIcon={<Wand2 size={14} />}
            onClick={() => {
              closeSettings();
              openWizard(0);
            }}
          >
            {t("settings:general.runWizard")}
          </Button>
        </SettingRow>
      </SettingGroup>
    </div>
  );
}
