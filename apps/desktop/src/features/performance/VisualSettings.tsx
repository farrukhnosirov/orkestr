/**
 * Settings → Interface: Performance Mode (§31) with what each mode allows,
 * automatic switching, density and background (§29 shell preferences), the
 * HUD (§34) and the history of interface changes with undo / redo.
 */
import type { PerformanceMode, UiPatchOp } from "@orkestr/protocol";
import { Button } from "@orkestr/ui";
import { clsx } from "clsx";
import { BatteryCharging, BatteryMedium, PanelTopClose, PictureInPicture2, PlugZap, Redo2, Undo2 } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { formatPercent, useT } from "../../i18n";
import { useAiUiStore, useUiPrefs } from "../aiui/aiuiStore";
import { canRedo, canUndo } from "../aiui/undo";
import { HUD_SHORTCUT } from "../hud/commands";
import { hudControls } from "../hud/hudWindow";
import { Segmented, SettingGroup, SettingRow, Switch } from "../settings/controls";
import type { SettingsSectionProps } from "../settings/sections";
import { useSettingsStore } from "../settings/settingsStore";
import { usePerfStore } from "./perfStore";
import { PERF_MODES, resolvePolicy, type PerfPolicy } from "./policy";
import "./performance.css";

/** Procedural glyph: four bars, more lit for richer modes. */
function ModeGlyph({ mode }: { mode: PerformanceMode }) {
  const level = 4 - PERF_MODES.indexOf(mode);
  return (
    <svg className="perf-glyph" viewBox="0 0 40 28" aria-hidden="true">
      {[0, 1, 2, 3].map((i) => (
        <rect
          key={i}
          x={4 + i * 9}
          y={20 - i * 5}
          width={6}
          height={4 + i * 5}
          rx={1.5}
          className={clsx("perf-glyph__bar", i < level && "perf-glyph__bar--on")}
        />
      ))}
    </svg>
  );
}

function PolicyTable({ policy }: { policy: PerfPolicy }) {
  const { t } = useT("performance");
  const onOff = (v: boolean) => (v ? t("policy.on") : t("policy.off"));
  const rows: [string, string][] = [
    [t("policy.blur"), onOff(policy.blur)],
    [t("policy.glow"), onOff(policy.glow)],
    [t("policy.motion"), t(`policy.motionLevels.${policy.motion}`)],
    [t("policy.background"), onOff(policy.backgroundAnimation)],
    [t("policy.robots"), t("policy.robotsValue", { fps: policy.robotFps, dimension: policy.robots3d ? t("policy.dimension3d") : t("policy.dimension2d") })],
    [t("policy.charts"), t("policy.chartsValue", { seconds: Math.round(policy.chartRefreshMs / 1000) })],
  ];
  return (
    <dl className="perf-policy" aria-label={t("policy.title")}>
      {rows.map(([k, v]) => (
        <div key={k} className="perf-policy__row">
          <dt>{k}</dt>
          <dd>{v}</dd>
        </div>
      ))}
    </dl>
  );
}

/** A range input that saves when the user stops moving it. */
function DeferredSlider({ value, min, max, step, label, onCommit, format }: { value: number; min: number; max: number; step: number; label: string; onCommit: (v: number) => void; format: (v: number) => string }) {
  const [local, setLocal] = useState(value);
  const timer = useRef(0);
  useEffect(() => setLocal(value), [value]);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  return (
    <div className="perf-slider">
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={local}
        aria-label={label}
        aria-valuetext={format(local)}
        onChange={(e) => {
          const v = Number(e.target.value);
          setLocal(v);
          window.clearTimeout(timer.current);
          timer.current = window.setTimeout(() => {
            if (v !== value) onCommit(v);
          }, 450);
        }}
        style={{ ["--perf-fill" as string]: `${((local - min) / (max - min)) * 100}%` }}
      />
      <span className="perf-slider__value">{format(local)}</span>
    </div>
  );
}

function History() {
  const { t } = useT("aiui");
  const undo = useAiUiStore((s) => s.undo);
  const busy = useAiUiStore((s) => s.busy);
  const error = useAiUiStore((s) => s.error);
  const { undoLast, redoLast } = useAiUiStore.getState();
  const who = (a: string) => (a === "Manager" ? t("history.byManager") : a === "Agent" ? t("history.byAgent") : t("history.byUser"));
  const past = [...undo.past].reverse();
  const future = [...undo.future].reverse();
  return (
    <SettingGroup title={t("history.title")}>
      <p className="perf-note">{t("history.description")}</p>
      {past.length + future.length === 0 ? (
        <p className="perf-note perf-note--muted">{t("history.empty")}</p>
      ) : (
        <ul className="aiui-history">
          {future.map((e) => (
            <li key={`f-${e.id}-${e.at}`} className="aiui-history__item aiui-history__item--undone">
              <span className="aiui-history__summary">{e.summary}</span>
              <span className="aiui-history__who">{who(e.author)}</span>
            </li>
          ))}
          {past.map((e) => (
            <li key={`p-${e.id}-${e.at}`} className="aiui-history__item">
              <span className="aiui-history__summary">{e.summary}</span>
              <span className="aiui-history__who">{who(e.author)}</span>
            </li>
          ))}
        </ul>
      )}
      <div className="aiui-history__actions">
        <Button size="sm" leftIcon={<Undo2 size={13} />} disabled={!canUndo(undo) || busy} onClick={() => void undoLast()}>
          {t("history.undo")}
        </Button>
        <Button size="sm" leftIcon={<Redo2 size={13} />} disabled={!canRedo(undo) || busy} onClick={() => void redoLast()}>
          {t("history.redo")}
        </Button>
      </div>
      {error && <p className="perf-note perf-note--error">{error}</p>}
      <p className="perf-note perf-note--muted">{t("history.tryIt")}</p>
    </SettingGroup>
  );
}

export function VisualSettings(_props: SettingsSectionProps) {
  const { t } = useT(["performance", "hud", "aiui"]);
  const settings = useSettingsStore((s) => s.settings);
  const saving = useSettingsStore((s) => s.saving);
  const update = useSettingsStore((s) => s.update);
  const perf = usePerfStore();
  const prefs = useUiPrefs();
  const aiBusy = useAiUiStore((s) => s.busy);
  const applyUserPatch = useAiUiStore((s) => s.applyUserPatch);

  const chosen = settings?.performanceMode ?? perf.chosen;
  const preview = resolvePolicy(chosen, { reducedMotion: perf.reducedMotion, hidden: false });

  const patchPref = (op: UiPatchOp, summary: string) => void applyUserPatch([op], summary);
  const pct = (v: number) => formatPercent(v);

  const power = perf.power;
  const powerText = !power?.supported
    ? t("performance:power.unsupported")
    : power.hasBattery && power.onAcPower === false
      ? power.batteryPercent != null
        ? t("performance:power.onBattery", { percent: pct(power.batteryPercent / 100) })
        : t("performance:power.onBatteryUnknown")
      : t("performance:power.onAc");
  const PowerIcon = !power?.supported ? PlugZap : power.onAcPower === false ? BatteryMedium : BatteryCharging;

  return (
    <div className="perf-settings">
      <SettingGroup title={t("performance:group.mode")}>
        <div className="perf-modes" role="radiogroup" aria-label={t("performance:group.mode")}>
          {PERF_MODES.map((m) => {
            const selected = m === chosen;
            return (
              <button
                key={m}
                type="button"
                role="radio"
                aria-checked={selected}
                disabled={!settings || saving === "performanceMode"}
                className={clsx("perf-mode", `perf-mode--${m}`, selected && "perf-mode--active", perf.mode === m && !selected && "perf-mode--effective")}
                onClick={() => void update({ performanceMode: m })}
              >
                <ModeGlyph mode={m} />
                <span className="perf-mode__name">{t(`performance:modes.${m}.name`)}</span>
                <span className="perf-mode__desc">{t(`performance:modes.${m}.description`)}</span>
              </button>
            );
          })}
        </div>
        <div className="perf-effective" aria-live="polite">
          <span className="perf-effective__label">{t("performance:effective.label")}</span>
          <strong>{t(`performance:modes.${perf.mode}.name`)}</strong>
          <span className="perf-effective__reason">· {t(`performance:effective.${perf.reason}`)}</span>
          <span className="perf-effective__power">
            <PowerIcon size={13} aria-hidden="true" /> {powerText}
          </span>
        </div>
        {perf.reducedMotion && <p className="perf-note perf-note--muted">{t("performance:reducedMotion")}</p>}
        <div className="perf-policy-wrap">
          <div className="perf-policy__title">{t("performance:policy.title")}</div>
          <PolicyTable policy={preview} />
        </div>
      </SettingGroup>

      <SettingGroup title={t("performance:group.auto")}>
        <SettingRow title={t("performance:autoBattery")} description={t("performance:autoBatteryDesc")}>
          <Switch checked={perf.autoBattery} onChange={perf.setAutoBattery} label={t("performance:autoBattery")} />
        </SettingRow>
        <SettingRow title={t("performance:autoDegrade")} description={t("performance:autoDegradeDesc")}>
          <Switch checked={perf.autoDegrade} onChange={perf.setAutoDegrade} label={t("performance:autoDegrade")} />
        </SettingRow>
      </SettingGroup>

      <SettingGroup title={t("performance:group.density")}>
        <SettingRow title={t("performance:density")} description={t("performance:densityDesc")}>
          <Segmented
            label={t("performance:density")}
            value={prefs.density}
            disabled={aiBusy}
            options={[
              { value: "Comfortable", label: t("performance:densityOptions.Comfortable") },
              { value: "Compact", label: t("performance:densityOptions.Compact") },
            ]}
            onChange={(v) =>
              patchPref({ op: "replace", path: "/prefs/density", value: v }, t("aiui:summaries.density", { value: t(`aiui:density.${v}`) }))
            }
          />
        </SettingRow>
        <SettingRow title={t("performance:backgroundDim")} description={t("performance:backgroundDimDesc")}>
          <DeferredSlider
            value={prefs.backgroundDim}
            min={0}
            max={0.9}
            step={0.05}
            label={t("performance:backgroundDim")}
            format={pct}
            onCommit={(v) => patchPref({ op: "replace", path: "/prefs/backgroundDim", value: v }, t("aiui:summaries.backgroundDim", { percent: pct(v) }))}
          />
        </SettingRow>
      </SettingGroup>

      <SettingGroup title={t("performance:group.hud")}>
        {hudControls.available() ? (
          <div className="perf-hud-actions">
            <Button size="sm" leftIcon={<PictureInPicture2 size={13} />} onClick={() => void hudControls.show()}>
              {t("hud:settings.open")}
            </Button>
            <Button size="sm" variant="ghost" leftIcon={<PanelTopClose size={13} />} onClick={() => void hudControls.collapse()}>
              {t("hud:settings.collapse")}
            </Button>
          </div>
        ) : (
          <p className="perf-note perf-note--muted">{t("hud:settings.browserOnly")}</p>
        )}
        <p className="perf-note perf-note--muted">{t("hud:settings.hotkey", { keys: HUD_SHORTCUT })}</p>
        <SettingRow title={t("hud:settings.opacity")} description={t("hud:settings.opacityDesc")}>
          <DeferredSlider
            value={prefs.hud.opacity}
            min={0.4}
            max={1}
            step={0.02}
            label={t("hud:settings.opacity")}
            format={pct}
            onCommit={(v) => patchPref({ op: "replace", path: "/prefs/hud/opacity", value: v }, t("aiui:summaries.hudOpacity", { percent: pct(v) }))}
          />
        </SettingRow>
        <SettingRow title={t("hud:settings.showAgents")} description={t("hud:settings.showAgentsDesc")}>
          <Switch
            checked={prefs.hud.showAgents}
            busy={aiBusy}
            label={t("hud:settings.showAgents")}
            onChange={(v) => patchPref({ op: "replace", path: "/prefs/hud/showAgents", value: v }, t("aiui:summaries.hudAgents"))}
          />
        </SettingRow>
        <SettingRow title={t("hud:settings.showInput")} description={t("hud:settings.showInputDesc")}>
          <Switch
            checked={prefs.hud.showManagerInput}
            busy={aiBusy}
            label={t("hud:settings.showInput")}
            onChange={(v) => patchPref({ op: "replace", path: "/prefs/hud/showManagerInput", value: v }, t("aiui:summaries.hudInput"))}
          />
        </SettingRow>
      </SettingGroup>

      <History />
    </div>
  );
}
