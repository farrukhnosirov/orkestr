/**
 * Theme editor tab bodies. Each edits the draft through `edit(fn)`; the
 * store validates and only valid drafts reach the live preview.
 */
import type { ThemeBackground, ThemeConfig } from "@orkestr/protocol";
import { MAX_BACKGROUND_COLORS } from "@orkestr/protocol";
import {
  contrastReport,
  deriveAccentTokens,
  isValidTokenValue,
  matchesKind,
  parseColor,
  tokenKind,
} from "@orkestr/ui";
import { clsx } from "clsx";
import { Check, Play, Plus, Sparkles, X } from "lucide-react";
import { useMemo, useState } from "react";

import { useFormat, useT } from "../../i18n";
import { Segmented, Switch } from "../settings/controls";
import { ColorField, ColorPicker, Select, Slider } from "./controls";
import { ORB_PAD } from "../orb/model";
import { OrbSphere } from "../orb/OrbSphere";
import {
  COLOR_GROUPS,
  EASING_PRESETS,
  FONT_SIZES,
  FONT_WEIGHTS,
  GRADIENT_TOKENS,
  MONO_FONTS,
  RADII,
  SANS_FONTS,
  SHADOW_PRESETS,
  SOUND_EVENTS,
  SOUND_PRESETS,
  SPACES,
  type ShadowPreset,
  familyScale,
  shadowPresetOf,
  withFamilyScale,
  withToken,
} from "./logic";
import { BackgroundCanvas, useEffectiveVisuals, useThemeRevision } from "./ThemeRuntime";
import { playThemePreset } from "../sound/engine";
import { THEME_SOUND_EVENTS } from "../sound/recipes";

export type Edit = (fn: (t: ThemeConfig) => ThemeConfig) => void;

export interface TabProps {
  draft: ThemeConfig;
  edit: Edit;
}

/** Current value of a token: the draft's override, else what `:root` resolves now. */
function useTokenValue(draft: ThemeConfig): (name: string) => string {
  useThemeRevision();
  const css = getComputedStyle(document.documentElement);
  return (name) => draft.tokens[name] ?? css.getPropertyValue(`--ok-${name}`).trim();
}

const setToken = (edit: Edit, name: string, value: string | null) => edit((t) => ({ ...t, tokens: withToken(t.tokens, name, value) }));

// ---------------------------------------------------------------------------
// Colors
// ---------------------------------------------------------------------------

export function ColorsTab({ draft, edit }: TabProps) {
  const { t } = useT("theme");
  const value = useTokenValue(draft);
  const palette = useMemo(
    () =>
      ["color-accent", "color-violet", "color-magenta", "color-success", "color-warning", "color-danger", "color-info", "color-text"]
        .map((k) => value(k))
        .filter((v) => parseColor(v) !== null),
    [value],
  );
  const invalid = (name: string) => {
    const v = draft.tokens[name];
    if (v === undefined) return false;
    const kind = tokenKind(name);
    return !isValidTokenValue(v) || (kind !== null && !matchesKind(kind, v));
  };
  return (
    <div className="th-tab">
      <ContrastPanel draft={draft} />
      {COLOR_GROUPS.map((g) => (
        <section key={g.id} className="th-block">
          <h4 className="th-block__title">{t(`groups.${g.id}` as "groups.text")}</h4>
          {g.tokens.map((name) => (
            <ColorField
              key={name}
              label={t(`tokens.${name}` as "tokens.color-bg")}
              value={value(name)}
              overridden={name in draft.tokens}
              invalid={invalid(name)}
              swatches={palette}
              onChange={(v) => setToken(edit, name, v)}
              onReset={() => setToken(edit, name, null)}
              extra={
                name === "color-accent" ? (
                  <button
                    type="button"
                    className="th-color__derive"
                    title={t("editor.deriveAccent")}
                    aria-label={t("editor.deriveAccent")}
                    onClick={() => edit((th) => ({ ...th, tokens: { ...th.tokens, ...deriveAccentTokens(value("color-accent"), th.mode) } }))}
                  >
                    <Sparkles size={12} />
                  </button>
                ) : undefined
              }
            />
          ))}
        </section>
      ))}
      <section className="th-block">
        <h4 className="th-block__title">{t("groups.gradients")}</h4>
        {GRADIENT_TOKENS.map((name) => (
          <GradientField key={name} name={name} draft={draft} edit={edit} current={value(name)} />
        ))}
        <button
          type="button"
          className="th-link"
          onClick={() =>
            edit((th) => ({
              ...th,
              tokens: {
                ...th.tokens,
                "gradient-accent": `linear-gradient(135deg, ${value("color-accent")} 0%, ${value("color-violet")} 100%)`,
              },
            }))
          }
        >
          <Sparkles size={12} /> {t("editor.gradientFromAccents")}
        </button>
      </section>
    </div>
  );
}

function GradientField({ name, draft, edit, current }: { name: string; draft: ThemeConfig; edit: Edit; current: string }) {
  const { t } = useT("theme");
  const [text, setText] = useState<string | null>(null);
  const shown = text ?? current;
  const ok = isValidTokenValue(shown) && matchesKind("Gradient", shown);
  return (
    <div className={clsx("th-gradient", !ok && "th-gradient--invalid")}>
      <div className="th-gradient__head">
        <span className="th-color__label">
          {t(`tokens.${name}` as "tokens.color-bg")}
          {name in draft.tokens && <span className="th-color__dot" title={t("editor.overridden")} />}
        </span>
        <button type="button" className="th-link" disabled={!(name in draft.tokens)} onClick={() => {
          setText(null);
          setToken(edit, name, null);
        }}>
          {t("editor.resetShort")}
        </button>
      </div>
      <div className="th-gradient__preview" style={{ background: ok ? shown : undefined }} />
      <textarea
        className="th-code th-code--small"
        aria-label={t(`tokens.${name}` as "tokens.color-bg")}
        spellCheck={false}
        rows={2}
        value={shown}
        onChange={(e) => {
          setText(e.target.value);
          if (isValidTokenValue(e.target.value) && matchesKind("Gradient", e.target.value)) setToken(edit, name, e.target.value);
        }}
        onBlur={() => setText(null)}
      />
      {!ok && <span className="th-error">{t("editor.invalidGradient")}</span>}
    </div>
  );
}

function ContrastPanel({ draft }: { draft: ThemeConfig }) {
  const { t } = useT("theme");
  const fmt = useFormat();
  const report = contrastReport(draft);
  const failing = report.filter((c) => !c.pass);
  return (
    <section className={clsx("th-contrast", failing.length > 0 && "th-contrast--warn")} aria-label={t("contrast.title")}>
      <div className="th-contrast__head">
        <b>{t("contrast.title")}</b>
        <span>{failing.length === 0 ? t("contrast.allPass") : t("contrast.failing", { count: failing.length })}</span>
      </div>
      <ul className="th-contrast__list">
        {report.map((c) => (
          <li key={c.id} className={clsx("th-contrast__item", !c.pass && (c.hard ? "th-contrast__item--fail" : "th-contrast__item--warn"))}>
            <span className="th-contrast__pair" aria-hidden="true">
              <span style={{ background: `var(--ok-${c.bg})`, color: `var(--ok-${c.fg})` }}>{t("contrast.sample")}</span>
            </span>
            <span className="th-contrast__name">{t(`contrast.rules.${c.id}` as "contrast.rules.text-bg")}</span>
            <span className="th-contrast__ratio">
              {t("contrast.ratio", { ratio: fmt.number(c.ratio, { maximumFractionDigits: 2 }), min: fmt.number(c.min) })}
            </span>
            {c.pass ? <Check size={13} className="th-contrast__ok" aria-label={t("contrast.pass")} /> : <X size={13} aria-label={c.hard ? t("contrast.blocking") : t("contrast.warning")} />}
          </li>
        ))}
      </ul>
      {failing.some((c) => c.hard) && <p className="th-error">{t("contrast.blockingHint")}</p>}
    </section>
  );
}

// ---------------------------------------------------------------------------
// Typography
// ---------------------------------------------------------------------------

function fontOptions(list: typeof SANS_FONTS, current: string, t: (k: string) => string) {
  const opts = list.map((f) => ({ value: f.stack, label: t(`fonts.${f.id}`) }));
  if (!list.some((f) => f.stack === current)) opts.unshift({ value: current, label: t("fonts.current") });
  return opts;
}

export function TypographyTab({ draft, edit }: TabProps) {
  const { t } = useT("theme");
  const fmt = useFormat();
  const value = useTokenValue(draft);
  const tt = (k: string) => t(k as never);
  const sizeScale = familyScale(draft.tokens, FONT_SIZES);
  const lineHeight = Number(value("line-height")) || 1.5;
  return (
    <div className="th-tab">
      <section className="th-block">
        <h4 className="th-block__title">{t("groups.families")}</h4>
        {(["font-sans", "font-display", "font-mono"] as const).map((name) => (
          <label key={name} className="th-field">
            <span className="th-field__label">{t(`tokens.${name}` as "tokens.color-bg")}</span>
            <Select
              label={t(`tokens.${name}` as "tokens.color-bg")}
              value={value(name)}
              options={fontOptions(name === "font-mono" ? MONO_FONTS : SANS_FONTS, value(name), tt)}
              onChange={(v) => setToken(edit, name, v)}
            />
            <span className="th-font-sample" style={{ fontFamily: value(name) }}>
              {t("editor.fontSample")}
            </span>
          </label>
        ))}
      </section>
      <section className="th-block">
        <h4 className="th-block__title">{t("groups.scale")}</h4>
        <Slider
          label={t("editor.textSize")}
          value={sizeScale}
          min={0.85}
          max={1.3}
          step={0.05}
          format={(v) => fmt.percent(v)}
          onChange={(v) => edit((th) => ({ ...th, tokens: withFamilyScale(th.tokens, FONT_SIZES, v) }))}
        />
        <Slider
          label={t("tokens.line-height")}
          value={lineHeight}
          min={1.2}
          max={1.8}
          step={0.05}
          format={(v) => fmt.number(v, { maximumFractionDigits: 2 })}
          onChange={(v) => setToken(edit, "line-height", Math.abs(v - 1.5) < 0.001 ? null : String(v))}
        />
        {Object.entries(FONT_WEIGHTS).map(([name, base]) => (
          <Slider
            key={name}
            label={t(`tokens.${name}` as never)}
            value={Number(value(name)) || base}
            min={300}
            max={800}
            step={100}
            onChange={(v) => setToken(edit, name, v === base ? null : String(v))}
          />
        ))}
      </section>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Effects (shape, glass, glow, shadows)
// ---------------------------------------------------------------------------

export function EffectsTab({ draft, edit }: TabProps) {
  const { t } = useT("theme");
  const fmt = useFormat();
  const fx = draft.effects;
  const shadow = shadowPresetOf(draft.tokens);
  const setFx = (patch: Partial<ThemeConfig["effects"]>) => edit((th) => ({ ...th, effects: { ...th.effects, ...patch } }));
  return (
    <div className="th-tab">
      <section className="th-block">
        <h4 className="th-block__title">{t("groups.shape")}</h4>
        <Slider
          label={t("editor.radius")}
          value={familyScale(draft.tokens, RADII)}
          min={0}
          max={2}
          step={0.05}
          format={(v) => fmt.percent(v)}
          onChange={(v) => edit((th) => ({ ...th, tokens: withFamilyScale(th.tokens, RADII, v) }))}
        />
        <Slider
          label={t("editor.spacing")}
          value={familyScale(draft.tokens, SPACES)}
          min={0.75}
          max={1.5}
          step={0.05}
          format={(v) => fmt.percent(v)}
          onChange={(v) => edit((th) => ({ ...th, tokens: withFamilyScale(th.tokens, SPACES, v) }))}
        />
      </section>
      <section className="th-block">
        <h4 className="th-block__title">{t("groups.glass")}</h4>
        <Slider label={t("editor.blur")} value={fx.blurPx} min={0} max={64} step={1} format={(v) => t("units.px", { value: v })} onChange={(v) => setFx({ blurPx: v })} />
        <Slider label={t("editor.panelOpacity")} value={fx.panelOpacity} min={0.2} max={1} step={0.01} format={(v) => fmt.percent(v)} onChange={(v) => setFx({ panelOpacity: v })} />
        <Slider label={t("editor.glow")} value={fx.glowIntensity} min={0} max={1} step={0.01} format={(v) => fmt.percent(v)} onChange={(v) => setFx({ glowIntensity: v })} />
      </section>
      <section className="th-block">
        <h4 className="th-block__title">{t("groups.shadows")}</h4>
        <Segmented<ShadowPreset | "custom">
          label={t("groups.shadows")}
          value={shadow ?? "custom"}
          options={[
            ...(["none", "soft", "medium", "deep"] as const).map((v) => ({ value: v, label: t(`shadows.${v}`) })),
            ...(shadow === null ? [{ value: "custom" as const, label: t("shadows.custom"), disabled: true }] : []),
          ]}
          onChange={(v) => {
            if (v === "custom") return;
            edit((th) => {
              const tokens = { ...th.tokens };
              for (const k of Object.keys(SHADOW_PRESETS.medium)) delete tokens[k];
              return { ...th, tokens: v === "medium" ? tokens : { ...tokens, ...SHADOW_PRESETS[v] } };
            });
          }}
        />
        <div className="th-shadow-demo" aria-hidden="true">
          <span />
        </div>
      </section>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Motion
// ---------------------------------------------------------------------------

export function MotionTab({ draft, edit }: TabProps) {
  const { t } = useT("theme");
  const fmt = useFormat();
  const value = useTokenValue(draft);
  const easing = Object.entries(EASING_PRESETS).find(([, v]) => v === value("ease"))?.[0] ?? "custom";
  const setMotion = (patch: Partial<ThemeConfig["motion"]>) => edit((th) => ({ ...th, motion: { ...th.motion, ...patch } }));
  return (
    <div className="th-tab">
      <section className="th-block">
        <div className="th-row">
          <span className="th-field__label">{t("editor.animations")}</span>
          <Switch
            label={t("editor.animations")}
            checked={draft.effects.animations}
            onChange={(v) => edit((th) => ({ ...th, effects: { ...th.effects, animations: v } }))}
          />
        </div>
        <Slider
          label={t("editor.motionSpeed")}
          value={draft.motion.speed}
          min={0.5}
          max={2}
          step={0.05}
          format={(v) => t("units.times", { value: fmt.number(v, { maximumFractionDigits: 2 }) })}
          onChange={(v) => setMotion({ speed: v })}
        />
        <Slider
          label={t("editor.motionIntensity")}
          value={draft.motion.intensity}
          min={0}
          max={1}
          step={0.05}
          format={(v) => fmt.percent(v)}
          onChange={(v) => setMotion({ intensity: v })}
        />
        <label className="th-field">
          <span className="th-field__label">{t("tokens.ease")}</span>
          <Select
            label={t("tokens.ease")}
            value={easing}
            options={[
              ...Object.keys(EASING_PRESETS).map((k) => ({ value: k, label: t(`easings.${k}` as never) })),
              ...(easing === "custom" ? [{ value: "custom", label: t("easings.custom") }] : []),
            ]}
            onChange={(k) => {
              const v = EASING_PRESETS[k];
              if (v) setToken(edit, "ease", v === EASING_PRESETS["smooth"] ? null : v);
            }}
          />
        </label>
        <p className="th-hint">{t("editor.motionHint")}</p>
        <div className="th-motion-demo" aria-hidden="true">
          <span />
        </div>
      </section>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Background
// ---------------------------------------------------------------------------

const KINDS: readonly ThemeBackground["kind"][] = ["Solid", "Gradient", "Nebula", "Grid", "Particles"];

export function BackgroundTab({ draft, edit }: TabProps) {
  const { t } = useT("theme");
  const fmt = useFormat();
  const bg = draft.background;
  const visuals = useEffectiveVisuals(draft);
  const [editing, setEditing] = useState<number | null>(null);
  const setBg = (patch: Partial<ThemeBackground>) => edit((th) => ({ ...th, background: { ...th.background, ...patch } }));
  const previewVisuals = { ...visuals, background: { ...visuals.background, renderer: visuals.background.renderer === "css" ? ("css" as const) : ("canvas" as const), resolution: 0.75 } };
  return (
    <div className="th-tab">
      <div className={clsx("th-bgpreview", `th-bgpreview--${bg.kind.toLowerCase()}`)}>
        <BackgroundCanvas visuals={previewVisuals} className="th-bgpreview__canvas" />
        <span className="th-bgpreview__label">
          {t(`renderers.${visuals.background.renderer}`)} ·{" "}
          {visuals.background.animated ? t("effects.fps", { fps: visuals.background.fps }) : t("effects.static")}
        </span>
      </div>
      <section className="th-block">
        <Segmented
          label={t("editor.backgroundKind")}
          value={bg.kind}
          options={KINDS.map((k) => ({ value: k, label: t(`backgrounds.${k}`) }))}
          onChange={(k) => setBg({ kind: k })}
        />
        <p className="th-hint">{t(`backgroundHints.${bg.kind}`)}</p>
      </section>
      <section className="th-block">
        <h4 className="th-block__title">{t("editor.backgroundColors")}</h4>
        <div className="th-bgcolors">
          {bg.colors.map((c, i) => (
            <span key={`${i}-${c}`} className="th-bgcolor">
              <button
                type="button"
                className={clsx("th-color__swatch", editing === i && "th-color__swatch--active")}
                aria-label={t("picker.open", { name: c })}
                onClick={() => setEditing(editing === i ? null : i)}
              >
                <span style={{ background: c }} />
              </button>
              <button
                type="button"
                className="th-bgcolor__remove"
                aria-label={t("editor.removeColor", { value: c })}
                onClick={() => {
                  setEditing(null);
                  setBg({ colors: bg.colors.filter((_, j) => j !== i) });
                }}
              >
                <X size={10} />
              </button>
            </span>
          ))}
          {bg.colors.length < MAX_BACKGROUND_COLORS && (
            <button type="button" className="th-bgcolor__add" aria-label={t("editor.addColor")} onClick={() => setBg({ colors: [...bg.colors, "#6d8bff"] })}>
              <Plus size={14} />
            </button>
          )}
          {bg.colors.length === 0 && <span className="th-hint">{t("editor.colorsFromAccents")}</span>}
        </div>
        {editing !== null && bg.colors[editing] !== undefined && (
          <ColorPicker
            value={bg.colors[editing] ?? "#000000"}
            allowAlpha={false}
            onChange={(v) => setBg({ colors: bg.colors.map((c, j) => (j === editing ? v : c)) })}
          />
        )}
      </section>
      <section className="th-block">
        <Slider label={t("editor.intensity")} value={bg.intensity} min={0} max={1} step={0.01} format={(v) => fmt.percent(v)} onChange={(v) => setBg({ intensity: v })} />
        <Slider label={t("editor.speed")} value={bg.speed} min={0} max={2} step={0.05} format={(v) => t("units.times", { value: fmt.number(v, { maximumFractionDigits: 2 }) })} onChange={(v) => setBg({ speed: v })} />
        <Slider label={t("editor.density")} value={bg.density} min={0} max={1} step={0.01} format={(v) => fmt.percent(v)} onChange={(v) => setBg({ density: v })} />
        <div className="th-row">
          <span className="th-field__label">
            {t("editor.webgl")}
            <span className="th-hint">{t("editor.webglHint")}</span>
          </span>
          <Switch label={t("editor.webgl")} checked={bg.webgl} onChange={(v) => setBg({ webgl: v })} />
        </div>
      </section>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Sounds (mapping of theme events to Sound Engine presets; previews play through the engine)
// ---------------------------------------------------------------------------

export function SoundsTab({ draft, edit }: TabProps) {
  const { t } = useT("theme");
  const fmt = useFormat();
  const s = draft.sounds;
  const setSounds = (patch: Partial<ThemeConfig["sounds"]>) => edit((th) => ({ ...th, sounds: { ...th.sounds, ...patch } }));
  return (
    <div className="th-tab">
      <section className="th-block">
        <div className="th-row">
          <span className="th-field__label">{t("editor.soundsEnabled")}</span>
          <Switch label={t("editor.soundsEnabled")} checked={s.enabled} onChange={(v) => setSounds({ enabled: v })} />
        </div>
        <Slider label={t("editor.volume")} value={s.volume} min={0} max={1} step={0.01} format={(v) => fmt.percent(v)} disabled={!s.enabled} onChange={(v) => setSounds({ volume: v })} />
      </section>
      <section className="th-block">
        <h4 className="th-block__title">{t("editor.soundEvents")}</h4>
        {SOUND_EVENTS.map((ev) => (
          <label key={ev} className="th-field th-field--inline">
            <span className="th-field__label">{t(`soundEvents.${ev}` as never)}</span>
            <Select
              label={t(`soundEvents.${ev}` as never)}
              value={s.events[ev] ?? "none"}
              disabled={!s.enabled}
              options={SOUND_PRESETS.map((p) => ({ value: p, label: t(`soundPresets.${p}` as never) }))}
              onChange={(p) =>
                setSounds({
                  events: Object.fromEntries(
                    Object.entries({ ...s.events, [ev]: p }).filter(([, v]) => v !== "none"),
                  ),
                })
              }
            />
            <button
              type="button"
              className="th-icon-btn"
              disabled={!s.enabled || (s.events[ev] ?? "none") === "none"}
              aria-label={t("editor.soundPreview", { event: t(`soundEvents.${ev}` as never) })}
              title={t("editor.soundPreview", { event: t(`soundEvents.${ev}` as never) })}
              onClick={(e) => {
                e.preventDefault();
                const cue = THEME_SOUND_EVENTS[ev];
                if (cue) playThemePreset(s.events[ev] ?? "none", cue, s.volume);
              }}
            >
              <Play size={13} aria-hidden="true" />
            </button>
          </label>
        ))}
        <p className="th-hint">{t("editor.soundsHint")}</p>
      </section>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Orb (ADR-0045): accent of the floating orb (`theme.robot.accent`, schema name kept)
// ---------------------------------------------------------------------------

export function OrbTab({ draft, edit }: TabProps) {
  const { t } = useT("theme");
  const value = useTokenValue(draft);
  const r = draft.robot;
  const setAccent = (accent: string | null) => edit((th) => ({ ...th, robot: { ...th.robot, accent } }));
  return (
    <div className="th-tab">
      <div className="th-orb-preview" aria-hidden="true">
        <OrbSphere size={64} pad={ORB_PAD} mood="working" motion="static" fps={0} rings={2} level={0} themeKey={JSON.stringify(draft.tokens)} accent={r.accent} lit={false} />
      </div>
      <section className="th-block">
        <div className="th-row">
          <span className="th-field__label">{t("editor.robotUseAccent")}</span>
          <Switch label={t("editor.robotUseAccent")} checked={r.accent === null} onChange={(v) => setAccent(v ? null : value("color-accent"))} />
        </div>
        {r.accent !== null && <ColorPicker value={r.accent} allowAlpha={false} onChange={(v) => setAccent(v)} />}
        <p className="th-hint">{t("editor.orbHint")}</p>
      </section>
    </div>
  );
}
