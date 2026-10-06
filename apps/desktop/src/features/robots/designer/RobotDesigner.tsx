/**
 * Robot Designer (§30): edits a {@link RobotDesign} with a live preview that
 * cycles through every state (or pins one). 2D procedural SVG by default,
 * the low-poly 3D robot in Ultra Visual. Saving goes through the caller
 * (`robots.setConfig`), which validates again in the core.
 */
import type { PerformanceMode, RobotDesign } from "@orkestr/protocol";
import { Button, Input } from "@orkestr/ui";
import { clsx } from "clsx";
import { Check, Pause, Play, RotateCcw } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import { useFormat, useT } from "../../../i18n";
import { Segmented, Switch } from "../../settings/controls";
import {
  ACCESSORY_IDS,
  accentOf,
  BUBBLE_LEVELS,
  isHexColor,
  MATERIALS,
  MAX_ACCESSORIES,
  normalizeDesign,
  RENDER_MODES,
  SCALE_RANGE,
  SKIN_IDS,
  SKINS,
  SOUND_PRESET_IDS,
  toggleAccessory,
  validateDesign,
} from "../model/design";
import { previewStateAt, ROBOT_STATES, type RobotState, stateColor } from "../model/states";
import { RobotView } from "../render/RobotView";

export interface RobotDesignerProps {
  /** Saved design (the draft resets when it changes). */
  value: RobotDesign;
  mode: PerformanceMode;
  saving: boolean;
  onSave: (design: RobotDesign) => void;
  /** Template designs only: drop the override and use the default design. */
  onResetToDefault?: () => void;
}

function sameDesign(a: RobotDesign, b: RobotDesign): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

/** Live preview clock: cycles states unless one is pinned. */
function usePreviewState(pinned: RobotState | null, playing: boolean): RobotState {
  const [t, setT] = useState(0);
  useEffect(() => {
    if (pinned || !playing) return undefined;
    const start = performance.now() - t;
    const timer = window.setInterval(() => setT(performance.now() - start), 200);
    return () => window.clearInterval(timer);
    // `t` is only the resume point; restarting on every tick would reset the interval.
  }, [pinned, playing]); // eslint-disable-line react-hooks/exhaustive-deps
  return pinned ?? previewStateAt(t);
}

export function RobotDesigner({ value, mode, saving, onSave, onResetToDefault }: RobotDesignerProps) {
  const { t } = useT(["robots", "common"]);
  const fmt = useFormat();
  const [draft, setDraft] = useState<RobotDesign>(value);
  const [pinned, setPinned] = useState<RobotState | null>(null);
  const [playing, setPlaying] = useState(true);
  const valueKey = JSON.stringify(value);
  useEffect(() => setDraft(value), [valueKey]); // eslint-disable-line react-hooks/exhaustive-deps

  const state = usePreviewState(pinned, playing);
  const issues = useMemo(() => validateDesign(draft), [draft]);
  const dirty = !sameDesign(draft, value);
  const set = (patch: Partial<RobotDesign>) => setDraft((d) => normalizeDesign({ ...d, ...patch }));
  const setBehaviour = (patch: Partial<RobotDesign["behaviour"]>) => setDraft((d) => ({ ...d, behaviour: { ...d.behaviour, ...patch } }));
  const accent = accentOf(draft);
  const threeDBlocked = draft.mode === "ThreeD" && mode !== "UltraVisual";

  return (
    <div className="rd">
      <div className="rd__stage" style={{ ["--robot-color" as string]: stateColor(state, accent) }}>
        <div className="rd__stage-grid" aria-hidden="true" />
        <div className="rd__stage-glow" aria-hidden="true" />
        <div className="rd__robot">
          <RobotView design={draft} state={state} size={Math.round(190 * Math.min(1.15, draft.scale))} mode={mode} hovered />
        </div>
        <div className="rd__stage-bar">
          <span className="rd__state-name">
            <span className="rd__state-dot" aria-hidden="true" />
            {t(`common:status.agent.${state}`)}
          </span>
          <button
            type="button"
            className="rd__play"
            aria-label={playing && !pinned ? t("robots:designer.pause") : t("robots:designer.play")}
            onClick={() => {
              setPinned(null);
              setPlaying((p) => (pinned ? true : !p));
            }}
          >
            {playing && !pinned ? <Pause size={14} /> : <Play size={14} />}
          </button>
        </div>
        <div className="rd__states" role="radiogroup" aria-label={t("robots:designer.previewState")}>
          {ROBOT_STATES.map((s) => (
            <button
              key={s}
              type="button"
              role="radio"
              aria-checked={pinned === s}
              className={clsx("rd__chip", pinned === s && "rd__chip--on")}
              style={{ ["--chip" as string]: stateColor(s, accent) }}
              onClick={() => setPinned((p) => (p === s ? null : s))}
            >
              {t(`common:status.agent.${s}`)}
            </button>
          ))}
        </div>
      </div>

      <div className="rd__controls">
        <div className="rd__field">
          <span className="rd__label">{t("robots:designer.mode")}</span>
          <Segmented
            label={t("robots:designer.mode")}
            value={draft.mode}
            options={RENDER_MODES.map((m) => ({ value: m, label: t(`robots:designer.modes.${m}`) }))}
            onChange={(m) => set({ mode: m })}
          />
          {threeDBlocked && <p className="rd__hint">{t("robots:designer.threeDHint")}</p>}
        </div>

        <div className="rd__field">
          <span className="rd__label">{t("robots:designer.skin")}</span>
          <div className="rd__skins" role="radiogroup" aria-label={t("robots:designer.skin")}>
            {SKIN_IDS.map((id) => {
              const p = SKINS[id];
              return (
                <button
                  key={id}
                  type="button"
                  role="radio"
                  aria-checked={draft.skin === id}
                  title={t(`robots:skins.${id}`)}
                  aria-label={t(`robots:skins.${id}`)}
                  className={clsx("rd__skin", draft.skin === id && "rd__skin--on")}
                  style={{ background: `radial-gradient(circle at 35% 30%, ${p.bodyLight}, ${p.bodyDark} 70%)`, ["--ring" as string]: p.accent }}
                  onClick={() => set({ skin: id })}
                >
                  {draft.skin === id && <Check size={14} />}
                </button>
              );
            })}
          </div>
        </div>

        <div className="rd__field">
          <span className="rd__label">{t("robots:designer.material")}</span>
          <Segmented
            label={t("robots:designer.material")}
            value={draft.material}
            options={MATERIALS.map((m) => ({ value: m, label: t(`robots:materials.${m}`) }))}
            onChange={(m) => set({ material: m })}
          />
        </div>

        <div className="rd__row">
          <label className="rd__field rd__field--inline">
            <span className="rd__label">{t("robots:designer.accent")}</span>
            <span className="rd__color">
              <input
                type="color"
                value={accent}
                aria-label={t("robots:designer.accent")}
                onChange={(e) => isHexColor(e.target.value) && set({ accent: e.target.value })}
              />
              <code>{accent}</code>
              {draft.accent !== null && (
                <button type="button" className="rd__link" onClick={() => setDraft((d) => ({ ...d, accent: null }))}>
                  {t("robots:designer.accentFromSkin")}
                </button>
              )}
            </span>
          </label>
        </div>

        <div className="rd__row">
          <label className="rd__field rd__field--inline">
            <span className="rd__label">
              {t("robots:designer.scale")} <output>{fmt.percent(draft.scale)}</output>
            </span>
            <input
              type="range"
              className="rd__range"
              min={SCALE_RANGE.min}
              max={SCALE_RANGE.max}
              step={0.05}
              value={draft.scale}
              onChange={(e) => set({ scale: Number(e.target.value) })}
            />
          </label>
          <label className="rd__field rd__field--inline">
            <span className="rd__label">
              {t("robots:designer.intensity")} <output>{fmt.percent(draft.animationIntensity)}</output>
            </span>
            <input
              type="range"
              className="rd__range"
              min={0}
              max={1}
              step={0.05}
              value={draft.animationIntensity}
              onChange={(e) => set({ animationIntensity: Number(e.target.value) })}
            />
          </label>
        </div>

        <div className="rd__field">
          <span className="rd__label">
            {t("robots:designer.accessories")}{" "}
            <span className="rd__muted">{t("robots:designer.accessoriesLimit", { count: MAX_ACCESSORIES })}</span>
          </span>
          <div className="rd__tags">
            {ACCESSORY_IDS.map((id) => {
              const on = draft.accessories.includes(id);
              const full = !on && draft.accessories.length >= MAX_ACCESSORIES;
              return (
                <button
                  key={id}
                  type="button"
                  aria-pressed={on}
                  disabled={full}
                  className={clsx("rd__tag", on && "rd__tag--on")}
                  onClick={() => set({ accessories: toggleAccessory(draft.accessories, id) })}
                >
                  {t(`robots:accessories.${id}`)}
                </button>
              );
            })}
          </div>
        </div>

        <div className="rd__field">
          <span className="rd__label">{t("robots:designer.voice")}</span>
          <div className="rd__inline">
            <Switch
              label={t("robots:designer.voiceEnabled")}
              checked={draft.voice.enabled}
              onChange={(enabled) => setDraft((d) => ({ ...d, voice: { ...d.voice, enabled } }))}
            />
            <span>{t("robots:designer.voiceEnabled")}</span>
          </div>
          <Input
            aria-label={t("robots:designer.voiceId")}
            value={draft.voice.voiceId ?? ""}
            maxLength={120}
            placeholder={t("robots:designer.voiceIdPlaceholder")}
            onChange={(e) => setDraft((d) => ({ ...d, voice: { ...d.voice, voiceId: e.target.value.trim() ? e.target.value : null } }))}
          />
        </div>

        <div className="rd__field">
          <span className="rd__label">{t("robots:designer.sounds")}</span>
          <Segmented
            label={t("robots:designer.sounds")}
            value={draft.sounds}
            options={SOUND_PRESET_IDS.map((s) => ({ value: s, label: t(`robots:sounds.${s}`) }))}
            onChange={(s) => set({ sounds: s })}
          />
        </div>

        <div className="rd__field">
          <span className="rd__label">{t("robots:designer.behaviour")}</span>
          <div className="rd__switches">
            {(["snap", "autoHideIdle", "clickThroughIdle"] as const).map((k) => (
              <div key={k} className="rd__inline">
                <Switch label={t(`robots:behaviour.${k}`)} checked={draft.behaviour[k]} onChange={(v) => setBehaviour({ [k]: v })} />
                <span>{t(`robots:behaviour.${k}`)}</span>
              </div>
            ))}
          </div>
          <span className="rd__sublabel">{t("robots:designer.bubble")}</span>
          <Segmented
            label={t("robots:designer.bubble")}
            value={draft.behaviour.bubble}
            options={BUBBLE_LEVELS.map((b) => ({ value: b, label: t(`robots:bubbleLevels.${b}`) }))}
            onChange={(bubble) => setBehaviour({ bubble })}
          />
        </div>

        {issues.length > 0 && (
          <p className="rd__error" role="alert">
            {t("robots:designer.invalid", { count: issues.length })}
          </p>
        )}

        <div className="rd__actions">
          {onResetToDefault && (
            <Button variant="ghost" leftIcon={<RotateCcw size={14} />} onClick={onResetToDefault} disabled={saving}>
              {t("robots:designer.useDefault")}
            </Button>
          )}
          <span className="rd__spacer" />
          <Button variant="ghost" disabled={!dirty || saving} onClick={() => setDraft(value)}>
            {t("robots:designer.revert")}
          </Button>
          <Button variant="primary" loading={saving} disabled={!dirty || issues.length > 0} onClick={() => onSave(normalizeDesign(draft))}>
            {t("common:actions.save")}
          </Button>
        </div>
      </div>
    </div>
  );
}
