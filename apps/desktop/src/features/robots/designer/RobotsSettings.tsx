/**
 * Settings → Robots: "Show floating robots", how many at once, how long a
 * finished robot lingers, and the Robot Designer for the default design or
 * one template's design (§30).
 */
import { Button, EmptyState, Spinner } from "@orkestr/ui";
import { AlertTriangle, Eye } from "lucide-react";
import { useEffect } from "react";

import { useT } from "../../../i18n";
import { templateName } from "../../../lib/coreText";
import { useTemplates } from "../../../panels/Agents/data";
import { Segmented, SettingGroup, SettingRow, Switch } from "../../settings/controls";
import type { SettingsSectionProps } from "../../settings/sections";
import { useSettingsStore } from "../../settings/settingsStore";
import { designForTemplate } from "../model/design";
import { robotShell } from "../shell";
import { type DesignerTarget, useRobotsStore } from "../store";
import { RobotDesigner } from "./RobotDesigner";
import "./designer.css";

const LINGER_PRESETS = ["0", "30", "90", "300", "900"] as const;
const MAX_VISIBLE_PRESETS = ["1", "3", "6", "9", "12"] as const;

function targetKey(target: DesignerTarget): string {
  return target.kind === "default" ? "" : target.templateId;
}

export function RobotsSettings(_props: SettingsSectionProps) {
  const { t } = useT(["robots", "common"]);
  const { config, loading, saving, error, hidden, target, load, patch, saveDesign, setTarget, unhideAll } = useRobotsStore();
  const mode = useSettingsStore((s) => s.settings?.performanceMode ?? "Balanced");
  const { templates } = useTemplates();

  useEffect(() => {
    if (!config && !loading) void load();
  }, [config, loading, load]);

  if (!config) {
    if (loading) return <div className="set-loading"><Spinner size={20} /> {t("robots:settings.loading")}</div>;
    return (
      <EmptyState
        icon={<AlertTriangle size={22} />}
        title={t("robots:settings.unavailable")}
        description={error ?? ""}
        action={<Button onClick={() => void load()}>{t("common:actions.retry")}</Button>}
      />
    );
  }

  const templateId = target.kind === "template" ? target.templateId : null;
  const design = designForTemplate(config, templateId);
  const hasOwn = templateId !== null && config.designs[templateId] !== undefined;
  const lingerOptions = (LINGER_PRESETS as readonly string[]).includes(String(config.lingerSeconds))
    ? LINGER_PRESETS
    : [...LINGER_PRESETS, String(config.lingerSeconds)];
  const maxOptions = (MAX_VISIBLE_PRESETS as readonly string[]).includes(String(config.maxVisible))
    ? MAX_VISIBLE_PRESETS
    : [...MAX_VISIBLE_PRESETS, String(config.maxVisible)].sort((a, b) => Number(a) - Number(b));
  const lingerLabel = (s: string) => {
    const n = Number(s);
    if (n === 0) return t("robots:settings.lingerNone");
    return n < 60 ? t("robots:settings.seconds", { count: n }) : t("robots:settings.minutes", { count: Math.round(n / 60) });
  };

  return (
    <div className="set-section">
      {error && (
        <div className="set-alert" role="alert">
          <AlertTriangle size={14} /> {error}
        </div>
      )}
      <SettingGroup title={t("robots:settings.floating")}>
        <SettingRow
          title={t("robots:settings.show")}
          description={robotShell.available() ? t("robots:settings.showDesc") : t("robots:settings.showDescBrowser")}
        >
          <Switch label={t("robots:settings.show")} checked={config.enabled} busy={saving} onChange={(enabled) => void patch({ enabled })} />
        </SettingRow>
        <SettingRow title={t("robots:settings.maxVisible")} description={t("robots:settings.maxVisibleDesc")}>
          <Segmented
            label={t("robots:settings.maxVisible")}
            value={String(config.maxVisible)}
            options={maxOptions.map((v) => ({ value: v, label: v }))}
            onChange={(v) => void patch({ maxVisible: Number(v) })}
          />
        </SettingRow>
        <SettingRow title={t("robots:settings.linger")} description={t("robots:settings.lingerDesc")}>
          <Segmented
            label={t("robots:settings.linger")}
            value={String(config.lingerSeconds)}
            options={lingerOptions.map((v) => ({ value: v, label: lingerLabel(v) }))}
            onChange={(v) => void patch({ lingerSeconds: Number(v) })}
          />
        </SettingRow>
        {hidden.size > 0 && (
          <SettingRow title={t("robots:settings.hidden", { count: hidden.size })} description={t("robots:settings.hiddenDesc")}>
            <Button leftIcon={<Eye size={14} />} onClick={unhideAll}>
              {t("robots:settings.showHidden")}
            </Button>
          </SettingRow>
        )}
      </SettingGroup>

      <SettingGroup title={t("robots:designer.title")}>
        <SettingRow title={t("robots:designer.target")} description={t("robots:designer.targetDesc")}>
          <select
            className="set-select"
            aria-label={t("robots:designer.target")}
            value={targetKey(target)}
            onChange={(e) => setTarget(e.target.value ? { kind: "template", templateId: e.target.value } : { kind: "default" })}
          >
            <option value="">{t("robots:designer.defaultDesign")}</option>
            {templates.map((tpl) => (
              <option key={tpl.id} value={tpl.id}>
                {config.designs[tpl.id] ? t("robots:designer.templateCustom", { name: templateName(tpl.name) }) : templateName(tpl.name)}
              </option>
            ))}
          </select>
        </SettingRow>
        <RobotDesigner
          key={targetKey(target)}
          value={design}
          mode={mode}
          saving={saving}
          onSave={(d) => void saveDesign(target, d)}
          {...(hasOwn ? { onResetToDefault: () => void saveDesign(target, null) } : {})}
        />
      </SettingGroup>
    </div>
  );
}
