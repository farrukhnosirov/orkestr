/**
 * Settings → Sounds and voice notifications (§33): the procedural sound
 * engine (presets, volume, DND muting, preview of every cue) and spoken
 * notifications (per-event toggles, quiet hours). Do Not Disturb itself is
 * the global setting in General.
 */
import type { SoundPreset, VoiceConfig } from "@orkestr/protocol";
import { Button, EmptyState, Input, Spinner } from "@orkestr/ui";
import { clsx } from "clsx";
import { AlertTriangle, BellOff, Volume2 } from "lucide-react";
import { useEffect, useState } from "react";

import { useFormat, useT } from "../../../i18n";
import { Segmented, SettingGroup, SettingRow, Switch } from "../../settings/controls";
import type { SettingsSectionProps } from "../../settings/sections";
import { useSettingsStore } from "../../settings/settingsStore";
import { playSound } from "../../sound/engine";
import { SOUND_CUES } from "../../sound/recipes";
import { VOICE_EVENT_KEYS } from "../notify/policy";
import { useVoiceStore } from "../store";
import { Notice, Slider } from "./fields";
import "../voice.css";

const PRESETS: readonly SoundPreset[] = ["Auto", "Nebula", "Crystal", "Pulse", "Minimal"];

export function SoundSection(_props: SettingsSectionProps) {
  const { t } = useT(["voice", "common"]);
  const fmt = useFormat();
  const { config, loading, error, load, patch } = useVoiceStore();
  const dnd = useSettingsStore((s) => s.settings?.doNotDisturb ?? false);
  const perf = useSettingsStore((s) => s.settings?.performanceMode ?? "Balanced");
  const [lastCue, setLastCue] = useState<string | null>(null);

  useEffect(() => {
    if (!config && !loading) void load();
  }, [config, loading, load]);

  if (!config) {
    if (loading) return <div className="set-loading"><Spinner size={20} /> {t("common:state.loading")}</div>;
    return (
      <EmptyState
        icon={<AlertTriangle size={22} />}
        title={t("settings.unavailable")}
        description={error ?? undefined}
        action={<Button onClick={() => void load()}>{t("common:actions.retry")}</Button>}
      />
    );
  }
  const up = (fn: (c: VoiceConfig) => VoiceConfig) => void patch(fn);
  const snd = config.sound;
  const n = config.notifications;

  return (
    <div className="set-section vx-settings">
      {error && <div className="set-alert">{error}</div>}
      {dnd && (
        <Notice tone="info" icon={<BellOff size={16} />} title={t("sound.dndOn")}>
          {t("sound.dndOnText")}
        </Notice>
      )}

      <SettingGroup title={t("sound.groups.sounds")}>
        <SettingRow title={t("sound.enabled")} description={t("sound.enabledDesc")}>
          <Switch label={t("sound.enabled")} checked={snd.enabled} onChange={(v) => up((c) => ({ ...c, sound: { ...c.sound, enabled: v } }))} />
        </SettingRow>
        <SettingRow title={t("sound.volume")}>
          <Slider
            value={snd.volumePct}
            min={0}
            max={100}
            step={5}
            label={t("sound.volume")}
            format={(v) => fmt.percent(v / 100)}
            disabled={!snd.enabled}
            onCommit={(v) => {
              up((c) => ({ ...c, sound: { ...c.sound, volumePct: v } }));
              window.setTimeout(() => playSound("completion", { test: true }), 120);
            }}
          />
        </SettingRow>
        <SettingRow title={t("sound.preset")} description={t("sound.presetDesc")} stacked>
          <Segmented
            label={t("sound.preset")}
            value={snd.preset}
            options={PRESETS.map((p) => ({ value: p, label: t(`sound.presets.${p}`) }))}
            onChange={(p) => {
              up((c) => ({ ...c, sound: { ...c.sound, preset: p } }));
              playSound("wake", { test: true, preset: p });
            }}
          />
        </SettingRow>
        <SettingRow title={t("sound.preview")} description={perf === "Performance" || perf === "BatterySaver" ? t("sound.previewLite") : t("sound.previewDesc")} stacked>
          <div className="vx-cues">
            {SOUND_CUES.map((cue) => (
              <button
                key={cue}
                type="button"
                className={clsx("vx-cue", lastCue === cue && "vx-cue--on")}
                disabled={!snd.enabled}
                onClick={() => {
                  setLastCue(cue);
                  playSound(cue, { test: true });
                }}
              >
                <Volume2 size={13} />
                {t(`sound.cues.${cue}`)}
              </button>
            ))}
          </div>
        </SettingRow>
        <SettingRow title={t("sound.muteInDnd")} description={t("sound.muteInDndDesc")}>
          <Switch label={t("sound.muteInDnd")} checked={snd.muteInDnd} onChange={(v) => up((c) => ({ ...c, sound: { ...c.sound, muteInDnd: v } }))} />
        </SettingRow>
      </SettingGroup>

      <SettingGroup title={t("sound.groups.notifications")}>
        <SettingRow title={t("notify.enabled")} description={t("notify.enabledDesc")}>
          <Switch
            label={t("notify.enabled")}
            checked={n.enabled}
            onChange={(v) => up((c) => ({ ...c, notifications: { ...c.notifications, enabled: v } }))}
          />
        </SettingRow>
        {VOICE_EVENT_KEYS.map((k) => (
          <SettingRow key={k} title={t(`notify.events.${k}`)} description={t(`notify.eventsDesc.${k}`)}>
            <Switch
              label={t(`notify.events.${k}`)}
              checked={n[k]}
              disabled={!n.enabled}
              onChange={(v) => up((c) => ({ ...c, notifications: { ...c.notifications, [k]: v } }))}
            />
          </SettingRow>
        ))}
        <SettingRow title={t("notify.quietHours")} description={t("notify.quietHoursDesc")}>
          <span className="vx-row-inline">
            <TimeInput
              label={t("notify.quietFrom")}
              value={n.quietHours.start}
              disabled={!n.quietHours.enabled}
              onCommit={(v) => up((c) => ({ ...c, notifications: { ...c.notifications, quietHours: { ...c.notifications.quietHours, start: v } } }))}
            />
            <span className="set-muted">—</span>
            <TimeInput
              label={t("notify.quietTo")}
              value={n.quietHours.end}
              disabled={!n.quietHours.enabled}
              onCommit={(v) => up((c) => ({ ...c, notifications: { ...c.notifications, quietHours: { ...c.notifications.quietHours, end: v } } }))}
            />
            <Switch
              label={t("notify.quietHours")}
              checked={n.quietHours.enabled}
              onChange={(v) => up((c) => ({ ...c, notifications: { ...c.notifications, quietHours: { ...c.notifications.quietHours, enabled: v } } }))}
            />
          </span>
        </SettingRow>
        <div className="set-muted set-muted--small">{t("notify.criticalNote")}</div>
      </SettingGroup>
    </div>
  );
}

function TimeInput({ value, label, disabled, onCommit }: { value: string; label: string; disabled: boolean; onCommit: (v: string) => void }) {
  const [v, setV] = useState(value);
  useEffect(() => setV(value), [value]);
  return (
    <Input
      type="time"
      className="vx-time"
      value={v}
      aria-label={label}
      disabled={disabled}
      onChange={(e) => setV(e.target.value)}
      onBlur={() => {
        if (/^\d{2}:\d{2}$/.test(v) && v !== value) onCommit(v);
        else setV(value);
      }}
    />
  );
}
