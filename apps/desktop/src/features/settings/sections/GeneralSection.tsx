import type { AppSettings, PerformanceMode } from "@orkestr/protocol";
import { BUILTIN_THEMES, Button, EmptyState, Input, Spinner } from "@orkestr/ui";
import { clsx } from "clsx";
import { AlertTriangle, Wand2 } from "lucide-react";
import { useEffect, useState } from "react";

import { useAppStore } from "../../../store/app";
import { useOnboardingStore } from "../../onboarding/onboardingStore";
import { Segmented, SettingGroup, SettingRow, Switch } from "../controls";
import type { SettingsSectionProps } from "../sections";
import { useSettingsStore } from "../settingsStore";

export const LANGUAGES: ReadonlyArray<{ value: string; label: string }> = [
  { value: "en", label: "English" },
  { value: "ru", label: "Русский" },
  { value: "de", label: "Deutsch" },
  { value: "es", label: "Español" },
];

export const RETENTION_PRESETS: ReadonlyArray<{ value: string; label: string }> = [
  { value: "30", label: "30 days" },
  { value: "90", label: "90 days" },
  { value: "180", label: "180 days" },
  { value: "365", label: "1 year" },
  { value: "0", label: "Forever" },
];

const PERFORMANCE: ReadonlyArray<{ value: PerformanceMode; label: string; title: string }> = [
  { value: "UltraVisual", label: "Ultra", title: "All effects on" },
  { value: "Balanced", label: "Balanced", title: "Default" },
  { value: "Performance", label: "Performance", title: "Minimal effects, more room for agents" },
  { value: "BatterySaver", label: "Battery", title: "Lowest power" },
];

/** Settings → General. */
export function GeneralSection(_props: SettingsSectionProps) {
  const { settings, loading, error, saving, autostartSupported, update, load } = useSettingsStore();
  const setThemeId = useAppStore((s) => s.setThemeId);
  const openWizard = useOnboardingStore((s) => s.openWizard);
  const closeSettings = useSettingsStore((s) => s.closeSettings);
  const [name, setName] = useState("");

  const profileName = settings?.profileName;
  useEffect(() => {
    if (profileName !== undefined) setName(profileName);
  }, [profileName]);

  if (!settings) {
    if (loading) return <div className="set-loading"><Spinner size={20} /> Loading settings…</div>;
    return (
      <EmptyState
        icon={<AlertTriangle size={22} />}
        title="Settings unavailable"
        description={error ?? "The core did not return settings."}
        action={<Button onClick={() => void load()}>Retry</Button>}
      />
    );
  }

  const set = <K extends keyof AppSettings>(key: K, value: AppSettings[K]) => void update({ [key]: value });
  const retention = String(settings.eventRetentionDays);
  const retentionOptions = RETENTION_PRESETS.some((p) => p.value === retention)
    ? RETENTION_PRESETS
    : [...RETENTION_PRESETS, { value: retention, label: `${retention} days` }];

  return (
    <div className="set-section">
      {error && (
        <div className="set-alert" role="alert">
          <AlertTriangle size={14} /> {error}
        </div>
      )}
      <SettingGroup title="Profile">
        <SettingRow title="Display name" description="Shown in the Manager chat and on approvals. Stored locally.">
          <Input
            aria-label="Display name"
            value={name}
            maxLength={80}
            placeholder="Your name"
            onChange={(e) => setName(e.target.value)}
            onBlur={() => name.trim() !== settings.profileName && set("profileName", name.trim())}
            onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()}
          />
        </SettingRow>
        <SettingRow title="Language" description="Interface language (translations arrive progressively).">
          <select
            className="set-select"
            aria-label="Language"
            value={settings.language}
            onChange={(e) => set("language", e.target.value)}
          >
            {LANGUAGES.map((l) => (
              <option key={l.value} value={l.value}>
                {l.label}
              </option>
            ))}
            {!LANGUAGES.some((l) => l.value === settings.language) && (
              <option value={settings.language}>{settings.language}</option>
            )}
          </select>
        </SettingRow>
      </SettingGroup>

      <SettingGroup title="Background">
        <SettingRow
          title="Keep running in the tray when closed"
          description="Closing the window hides Orkestr to the system tray so agents keep working. Quit from the tray menu."
        >
          <Switch
            label="Close to tray"
            checked={settings.closeToTray}
            busy={saving === "closeToTray"}
            onChange={(v) => set("closeToTray", v)}
          />
        </SettingRow>
        <SettingRow
          title="Start with your computer"
          description={
            autostartSupported
              ? "Starts Orkestr hidden in the tray when you sign in (opt-in, §12)."
              : "Autostart is not supported on this operating system."
          }
        >
          <Switch
            label="Start at login"
            checked={settings.autostart}
            disabled={!autostartSupported}
            busy={saving === "autostart"}
            onChange={(v) => set("autostart", v)}
          />
        </SettingRow>
      </SettingGroup>

      <SettingGroup title="Appearance & performance">
        <SettingRow title="Theme" stacked>
          <div className="set-themes" role="radiogroup" aria-label="Theme">
            {Object.values(BUILTIN_THEMES).map((t) => (
              <button
                key={t.id}
                type="button"
                role="radio"
                aria-checked={settings.themeId === t.id}
                className={clsx("set-theme", settings.themeId === t.id && "set-theme--active")}
                onClick={() => {
                  setThemeId(t.id);
                  set("themeId", t.id);
                }}
              >
                <span className={clsx("set-theme__swatch", `set-theme__swatch--${t.mode.toLowerCase()}`)} />
                {t.name}
              </button>
            ))}
          </div>
        </SettingRow>
        <SettingRow title="Performance mode" description="Trades visual effects for headroom for agents (§31)." stacked>
          <Segmented
            label="Performance mode"
            value={settings.performanceMode}
            options={PERFORMANCE}
            onChange={(v) => set("performanceMode", v)}
          />
        </SettingRow>
      </SettingGroup>

      <SettingGroup title="Notifications">
        <SettingRow title="Do not disturb" description="No pop-up toasts. Notifications are still collected in the bell.">
          <Switch
            label="Do not disturb"
            checked={settings.doNotDisturb}
            busy={saving === "doNotDisturb"}
            onChange={(v) => set("doNotDisturb", v)}
          />
        </SettingRow>
      </SettingGroup>

      <SettingGroup title="Data & privacy">
        <SettingRow
          title="Event history retention"
          description="Older events are pruned automatically. Approvals, decisions, emergency stops and rollbacks are always kept."
          stacked
        >
          <Segmented
            label="Event retention"
            value={retention}
            options={retentionOptions}
            onChange={(v) => set("eventRetentionDays", Number(v))}
          />
        </SettingRow>
        <SettingRow title="Usage telemetry" description="Orkestr collects no telemetry in this build. Everything stays on this machine.">
          <Switch label="Telemetry" checked={false} disabled onChange={() => {}} />
        </SettingRow>
      </SettingGroup>

      <SettingGroup title="Setup">
        <SettingRow title="Setup wizard" description="Walk through profile, storage, providers, permissions and system checks again.">
          <Button
            leftIcon={<Wand2 size={14} />}
            onClick={() => {
              closeSettings();
              openWizard(0);
            }}
          >
            Run setup again
          </Button>
        </SettingRow>
      </SettingGroup>
    </div>
  );
}
