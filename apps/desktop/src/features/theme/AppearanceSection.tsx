/**
 * Settings → Appearance (§29): theme gallery (built-in + user), per-device
 * selection, import/export, duplicate/delete, the editor entry point and the
 * Performance Mode with what it means for effects.
 */
import type { PerformanceMode } from "@orkestr/protocol";
import { Badge, Button, EmptyState, Spinner } from "@orkestr/ui";
import { AlertTriangle, Palette, Upload, Wand2 } from "lucide-react";
import { useRef, useState } from "react";

import { useT } from "../../i18n";
import { Segmented, SettingGroup, SettingRow, Switch } from "../settings/controls";
import type { SettingsSectionProps } from "../settings/sections";
import { useSettingsStore } from "../settings/settingsStore";
import { importThemeIntoEditor, openThemeEditor } from "./actions";
import { exportFileName } from "./logic";
import { contrastLevel, ThemeCard } from "./ThemeCard";
import { downloadText } from "./i18nText";
import { useVisualsStore } from "./ThemeRuntime";
import { useThemeStore } from "./themeStore";

const PERFORMANCE_MODES: readonly PerformanceMode[] = ["UltraVisual", "Balanced", "Performance", "BatterySaver"];

export function AppearanceSection(_props: SettingsSectionProps) {
  const { t } = useT(["theme", "settings", "common"]);
  const s = useThemeStore();
  const settings = useSettingsStore((x) => x.settings);
  const updateSettings = useSettingsStore((x) => x.update);
  const visuals = useVisualsStore((x) => x.visuals);
  const fileRef = useRef<HTMLInputElement>(null);
  const [notice, setNotice] = useState<{ tone: "success" | "danger"; text: string } | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);

  if (!s.loaded && s.themes.length === 0) {
    return (
      <div className="set-loading">
        <Spinner size={20} /> {t("theme:loading")}
      </div>
    );
  }
  if (!s.config) {
    return (
      <EmptyState
        icon={<AlertTriangle size={22} />}
        title={t("theme:unavailable")}
        description={s.error ?? undefined}
        action={<Button onClick={() => void s.refresh()}>{t("common:actions.retry")}</Button>}
      />
    );
  }

  const activeId = s.config.theme.id;
  const level = contrastLevel(s.contrast);
  const onImport = async (file: File) => {
    const res = importThemeIntoEditor(await file.text());
    setNotice(res.ok ? { tone: "success", text: t("theme:import.opened", { name: file.name }) } : { tone: "danger", text: t("theme:import.invalid", { issues: res.issues.slice(0, 3).join("; ") }) });
  };

  return (
    <div className="set-section th-appearance">
      <SettingGroup title={t("theme:gallery.title")}>
        <div className="th-gallery" role="radiogroup" aria-label={t("theme:gallery.title")}>
          {s.themes.map((entry) => (
            <ThemeCard
              key={entry.theme.id}
              theme={entry.theme}
              builtin={entry.builtin}
              active={entry.theme.id === activeId}
              busy={s.busy}
              onSelect={() => void s.selectTheme(entry.theme.id)}
              onEdit={() => openThemeEditor(entry.theme)}
              onDuplicate={() =>
                void s.duplicate(entry.theme.id, t("theme:gallery.copyName", { name: entry.theme.name })).then((e) => {
                  if (e) setNotice({ tone: "success", text: t("theme:gallery.duplicated", { name: e.theme.name }) });
                })
              }
              onExport={() => downloadText(exportFileName(entry.theme), s.exportJson(entry.theme))}
              onDelete={entry.builtin ? undefined : () => setConfirmDelete(entry.theme.id)}
            />
          ))}
        </div>
        {confirmDelete && (
          <div className="set-alert th-confirm" role="alertdialog" aria-label={t("theme:delete.title")}>
            <span>{t("theme:delete.confirm", { name: s.themes.find((e) => e.theme.id === confirmDelete)?.theme.name ?? confirmDelete })}</span>
            <span className="th-confirm__actions">
              <Button size="sm" variant="ghost" onClick={() => setConfirmDelete(null)}>
                {t("common:actions.cancel")}
              </Button>
              <Button
                size="sm"
                variant="danger"
                onClick={() => {
                  const id = confirmDelete;
                  setConfirmDelete(null);
                  void s.deleteTheme(id);
                }}
              >
                {t("theme:delete.action")}
              </Button>
            </span>
          </div>
        )}
        {notice && (
          <div className="th-notice" role="status">
            <Badge tone={notice.tone}>{notice.text}</Badge>
          </div>
        )}
        {s.error && (
          <div className="set-alert" role="alert">
            <AlertTriangle size={14} /> {s.error}
          </div>
        )}
        <div className="th-toolbar">
          <Button variant="primary" leftIcon={<Wand2 size={14} />} onClick={() => openThemeEditor()}>
            {t("theme:openEditor")}
          </Button>
          <Button leftIcon={<Upload size={14} />} onClick={() => fileRef.current?.click()}>
            {t("theme:import.button")}
          </Button>
          <input
            ref={fileRef}
            type="file"
            accept=".json,application/json"
            hidden
            onChange={(e) => {
              const f = e.target.files?.[0];
              e.target.value = "";
              if (f) void onImport(f);
            }}
          />
          <span className="th-toolbar__spacer" />
          <Badge tone={level === "pass" ? "success" : level === "warn" ? "warning" : "danger"} icon={<Palette size={12} />}>
            {t(`theme:contrast.summary.${level}`)}
          </Badge>
        </div>
      </SettingGroup>

      <SettingGroup title={t("theme:device.title")}>
        <SettingRow title={t("theme:device.perDevice")} description={t("theme:device.perDeviceDesc")}>
          <Switch label={t("theme:device.perDevice")} checked={s.scope === "Device"} busy={s.busy} onChange={(v) => void s.setPerDevice(v)} />
        </SettingRow>
      </SettingGroup>

      <SettingGroup title={t("theme:effects.title")}>
        <SettingRow title={t("settings:general.performanceMode")} description={t("theme:effects.performanceDesc")} stacked>
          <Segmented
            label={t("settings:general.performanceMode")}
            value={settings?.performanceMode ?? s.config.performanceMode}
            disabled={!settings}
            options={PERFORMANCE_MODES.map((value) => ({
              value,
              label: t(`settings:general.performance.${value}.label`),
              title: t(`settings:general.performance.${value}.title`),
            }))}
            onChange={(v) => void updateSettings({ performanceMode: v })}
          />
        </SettingRow>
        {visuals && (
          <SettingRow title={t("theme:effects.now")} stacked>
            <div className="th-facts">
              <span className="th-fact">
                <b>{t("theme:effects.renderer")}</b> {t(`theme:renderers.${visuals.background.renderer}`)}
              </span>
              <span className="th-fact">
                <b>{t("theme:effects.animation")}</b>{" "}
                {visuals.background.animated ? t("theme:effects.fps", { fps: visuals.background.fps }) : t("theme:effects.static")}
              </span>
              <span className="th-fact">
                <b>{t("theme:effects.blur")}</b> {t("theme:units.px", { value: visuals.blurPx })}
              </span>
              <span className="th-fact">
                <b>{t("theme:effects.motion")}</b> {t(`theme:motionLevels.${visuals.motion}`)}
              </span>
            </div>
          </SettingRow>
        )}
      </SettingGroup>
    </div>
  );
}
