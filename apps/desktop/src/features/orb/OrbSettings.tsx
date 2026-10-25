/**
 * Settings → Orb (ADR-0045): show the orb, its size, the count badge, the
 * animation level, reset its position, and a live preview of its states.
 * Values live in the UI configuration (`prefs.orb`, AI-editable, undoable).
 */
import type { OrbAnimation } from "@orkestr/protocol";
import { Button } from "@orkestr/ui";
import { LocateFixed } from "lucide-react";
import { useState } from "react";

import { useT } from "../../i18n";
import { useAiUiStore, useUiPrefs } from "../aiui/aiuiStore";
import { Segmented, SettingGroup, SettingRow, Switch } from "../settings/controls";
import type { SettingsSectionProps } from "../settings/sections";
import { activeTheme, useThemeStore } from "../theme/themeStore";
import { ORB_HUB_SHORTCUT, ORB_PAD, type OrbMood, orbPrefsOf } from "./model";
import { ANIMATIONS } from "./OrbMenu";
import { OrbSphere } from "./OrbSphere";
import { orbShell } from "./shell";
import { JARVIS_SIZES } from "../voice/jarvis/model";
import "./orb.css";

const SIZES = [56, 64, 72] as const;
const PREVIEW: readonly OrbMood[] = ["idle", "working", "approval", "error", "listening", "speaking"];

export function OrbSettings(_props: SettingsSectionProps) {
  const { t } = useT(["orb", "common"]);
  const prefs = orbPrefsOf(useUiPrefs().orb);
  const theme = useThemeStore(activeTheme);
  const [resetDone, setResetDone] = useState(false);
  const patch = (path: string, value: string | number | boolean, summary: string) =>
    void useAiUiStore.getState().applyUserPatch([{ op: "replace", path: `/prefs/orb/${path}`, value }], summary);
  const desktop = orbShell.available();

  return (
    <div className="orb-settings">
      <SettingGroup title={t("settings.group")}>
        <SettingRow title={t("settings.enabled")} description={desktop ? t("settings.enabledDesc", { shortcut: ORB_HUB_SHORTCUT }) : t("settings.enabledDescBrowser")}>
          <Switch
            label={t("settings.enabled")}
            checked={prefs.enabled}
            onChange={(v) => patch("enabled", v, v ? t("summaries.shown") : t("summaries.hidden"))}
          />
        </SettingRow>
        <SettingRow title={t("settings.size")} description={t("settings.sizeDesc")}>
          <Segmented<string>
            label={t("settings.size")}
            value={String(prefs.size)}
            options={SIZES.map((s) => ({ value: String(s), label: t(`settings.sizes.${s}`) }))}
            onChange={(v) => patch("size", Number(v), t("summaries.size", { size: v }))}
          />
        </SettingRow>
        <SettingRow title={t("settings.voiceSize")} description={t("settings.voiceSizeDesc")}>
          <Segmented<string>
            label={t("settings.voiceSize")}
            value={String(prefs.voiceSize)}
            options={JARVIS_SIZES.map((s) => ({ value: String(s), label: t(`settings.voiceSizes.${s}`) }))}
            onChange={(v) => patch("voiceSize", Number(v), t("summaries.voiceSize", { size: v }))}
          />
        </SettingRow>
        <SettingRow title={t("settings.badge")} description={t("settings.badgeDesc")}>
          <Switch label={t("settings.badge")} checked={prefs.showBadge} onChange={(v) => patch("showBadge", v, t("summaries.badge"))} />
        </SettingRow>
        <SettingRow title={t("settings.animation")} description={t("settings.animationDesc")}>
          <Segmented<OrbAnimation>
            label={t("settings.animation")}
            value={prefs.animation}
            options={ANIMATIONS.map((a) => ({ value: a, label: t(`animation.${a}`) }))}
            onChange={(v) => patch("animation", v, t("summaries.animation", { value: t(`animation.${v}`) }))}
          />
        </SettingRow>
        <SettingRow title={t("settings.position")} description={t("settings.positionDesc")}>
          <Button
            size="sm"
            leftIcon={<LocateFixed size={13} />}
            disabled={!desktop}
            onClick={() => {
              void orbShell.resetPosition().then(() => setResetDone(true));
            }}
          >
            {resetDone ? t("settings.positionReset") : t("settings.resetPosition")}
          </Button>
        </SettingRow>
      </SettingGroup>
      <SettingGroup title={t("settings.preview")}>
        <div className="orb-settings__preview">
          {PREVIEW.map((mood) => (
            <div key={mood}>
              <span className="orb-settings__swatch">
                <OrbSphere
                  size={prefs.size}
                  pad={ORB_PAD}
                  mood={mood}
                  motion="static"
                  fps={0}
                  rings={2}
                  level={0.6}
                  themeKey={`${theme.id}:${theme.mode}:${JSON.stringify(theme.tokens)}`}
                  accent={theme.robot?.accent ?? null}
                  lit={false}
                />
              </span>
              <span className="orb-settings__label">{t(`mood.${mood}`)}</span>
            </div>
          ))}
        </div>
      </SettingGroup>
    </div>
  );
}
