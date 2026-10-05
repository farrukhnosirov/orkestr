/**
 * Theme editor controls: a custom HSV color picker (saturation/value plane,
 * hue and alpha rails, hex field), a styled range slider and a select. All
 * keyboard accessible, styled only with `--ok-*` tokens (theme.css).
 */
import type { Rgba } from "@orkestr/protocol";
import { type Hsva, formatColor, hsvaToRgba, parseColor, rgbaToHsva, toHex } from "@orkestr/ui";
import { clsx } from "clsx";
import { RotateCcw } from "lucide-react";
import { type KeyboardEvent, type PointerEvent, type ReactNode, useEffect, useId, useRef, useState } from "react";

import { useT } from "../../i18n";

// ---------------------------------------------------------------------------
// Slider
// ---------------------------------------------------------------------------

export interface SliderProps {
  label: string;
  value: number;
  min: number;
  max: number;
  step: number;
  onChange: (v: number) => void;
  /** Formats the value read-out. */
  format?: (v: number) => string;
  disabled?: boolean;
}

export function Slider({ label, value, min, max, step, onChange, format, disabled = false }: SliderProps) {
  const id = useId();
  const pct = max > min ? ((value - min) / (max - min)) * 100 : 0;
  return (
    <div className={clsx("th-slider", disabled && "th-slider--disabled")}>
      <div className="th-slider__head">
        <label htmlFor={id} className="th-slider__label">
          {label}
        </label>
        <output htmlFor={id} className="th-slider__value">
          {format ? format(value) : String(value)}
        </output>
      </div>
      <input
        id={id}
        type="range"
        className="th-slider__input"
        min={min}
        max={max}
        step={step}
        value={value}
        disabled={disabled}
        style={{ ["--th-fill" as string]: `${pct}%` }}
        onChange={(e) => onChange(Number(e.target.value))}
      />
    </div>
  );
}

// ---------------------------------------------------------------------------
// Select
// ---------------------------------------------------------------------------

export interface SelectOption {
  value: string;
  label: string;
}

export function Select({
  label,
  value,
  options,
  onChange,
  disabled = false,
}: {
  label: string;
  value: string;
  options: readonly SelectOption[];
  onChange: (v: string) => void;
  disabled?: boolean;
}) {
  return (
    <select className="set-select th-select" aria-label={label} value={value} disabled={disabled} onChange={(e) => onChange(e.target.value)}>
      {options.map((o) => (
        <option key={o.value} value={o.value}>
          {o.label}
        </option>
      ))}
    </select>
  );
}

// ---------------------------------------------------------------------------
// Color picker
// ---------------------------------------------------------------------------

function useDrag(onMove: (x: number, y: number) => void) {
  const ref = useRef<HTMLDivElement>(null);
  const move = (e: PointerEvent<HTMLDivElement>) => {
    const el = ref.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    onMove(Math.min(1, Math.max(0, (e.clientX - r.left) / r.width)), Math.min(1, Math.max(0, (e.clientY - r.top) / r.height)));
  };
  return {
    ref,
    onPointerDown: (e: PointerEvent<HTMLDivElement>) => {
      e.currentTarget.setPointerCapture(e.pointerId);
      move(e);
    },
    onPointerMove: (e: PointerEvent<HTMLDivElement>) => {
      if (e.currentTarget.hasPointerCapture(e.pointerId)) move(e);
    },
  };
}

function arrowDelta(e: KeyboardEvent, step: number): [number, number] | null {
  const s = e.shiftKey ? step * 10 : step;
  if (e.key === "ArrowLeft") return [-s, 0];
  if (e.key === "ArrowRight") return [s, 0];
  if (e.key === "ArrowUp") return [0, -s];
  if (e.key === "ArrowDown") return [0, s];
  return null;
}

export interface ColorPickerProps {
  value: string;
  onChange: (value: string) => void;
  /** Quick swatches (theme palette). */
  swatches?: readonly string[];
  allowAlpha?: boolean;
}

/** HSV picker. Keeps hue while saturation is 0 (no jumping), emits hex or `rgba()`. */
export function ColorPicker({ value, onChange, swatches = [], allowAlpha = true }: ColorPickerProps) {
  const { t } = useT("theme");
  const [hsva, setHsva] = useState<Hsva>(() => rgbaToHsva(parseColor(value) ?? { r: 0, g: 0, b: 0, a: 1 }));
  const [hex, setHex] = useState(value);
  const last = useRef(value);

  // External changes (reset, JSON edit) re-sync the picker.
  useEffect(() => {
    if (value === last.current) return;
    last.current = value;
    const c = parseColor(value);
    if (c) setHsva((prev) => ({ ...rgbaToHsva(c), h: rgbaToHsva(c).s === 0 ? prev.h : rgbaToHsva(c).h }));
    setHex(value);
  }, [value]);

  const emit = (next: Hsva) => {
    setHsva(next);
    const out = formatColor(hsvaToRgba(allowAlpha ? next : { ...next, a: 1 }));
    last.current = out;
    setHex(out);
    onChange(out);
  };

  const sv = useDrag((x, y) => emit({ ...hsva, s: x, v: 1 - y }));
  const hue = useDrag((x) => emit({ ...hsva, h: x * 360 }));
  const alpha = useDrag((x) => emit({ ...hsva, a: Math.round(x * 100) / 100 }));
  const opaque: Rgba = hsvaToRgba({ ...hsva, a: 1 });
  const pure = toHex(hsvaToRgba({ h: hsva.h, s: 1, v: 1, a: 1 }));

  return (
    <div className="th-picker">
      <div
        {...sv}
        className="th-picker__plane"
        role="slider"
        tabIndex={0}
        aria-label={t("picker.plane")}
        aria-valuetext={`${Math.round(hsva.s * 100)}%, ${Math.round(hsva.v * 100)}%`}
        style={{ backgroundColor: pure }}
        onKeyDown={(e) => {
          const d = arrowDelta(e, 0.01);
          if (!d) return;
          e.preventDefault();
          emit({ ...hsva, s: Math.min(1, Math.max(0, hsva.s + d[0])), v: Math.min(1, Math.max(0, hsva.v - d[1])) });
        }}
      >
        <span className="th-picker__knob" style={{ left: `${hsva.s * 100}%`, top: `${(1 - hsva.v) * 100}%`, background: toHex(opaque) }} />
      </div>
      <div
        {...hue}
        className="th-picker__rail th-picker__rail--hue"
        role="slider"
        tabIndex={0}
        aria-label={t("picker.hue")}
        aria-valuemin={0}
        aria-valuemax={360}
        aria-valuenow={Math.round(hsva.h)}
        onKeyDown={(e) => {
          const d = arrowDelta(e, 1);
          if (!d) return;
          e.preventDefault();
          emit({ ...hsva, h: (hsva.h + d[0] - d[1] + 360) % 360 });
        }}
      >
        <span className="th-picker__thumb" style={{ left: `${(hsva.h / 360) * 100}%`, background: pure }} />
      </div>
      {allowAlpha && (
        <div
          {...alpha}
          className="th-picker__rail th-picker__rail--alpha"
          role="slider"
          tabIndex={0}
          aria-label={t("picker.alpha")}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={Math.round(hsva.a * 100)}
          style={{ ["--th-solid" as string]: toHex(opaque) }}
          onKeyDown={(e) => {
            const d = arrowDelta(e, 0.01);
            if (!d) return;
            e.preventDefault();
            emit({ ...hsva, a: Math.round(Math.min(1, Math.max(0, hsva.a + d[0] - d[1])) * 100) / 100 });
          }}
        >
          <span className="th-picker__thumb" style={{ left: `${hsva.a * 100}%`, background: formatColor(hsvaToRgba(hsva)) }} />
        </div>
      )}
      <div className="th-picker__row">
        <input
          className="th-picker__hex"
          aria-label={t("picker.value")}
          spellCheck={false}
          value={hex}
          onChange={(e) => {
            setHex(e.target.value);
            const c = parseColor(e.target.value);
            if (c) {
              setHsva(rgbaToHsva(c));
              last.current = e.target.value.trim();
              onChange(e.target.value.trim());
            }
          }}
        />
        {swatches.length > 0 && (
          <div className="th-picker__swatches">
            {swatches.slice(0, 8).map((s) => (
              <button
                key={s}
                type="button"
                className="th-picker__swatch"
                style={{ background: s }}
                aria-label={t("picker.useSwatch", { value: s })}
                title={s}
                onClick={() => {
                  const c = parseColor(s);
                  if (c) emit(rgbaToHsva(c));
                }}
              />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

/** One editable color token: swatch (opens the picker), label, value and reset. */
export function ColorField({
  label,
  hint,
  value,
  overridden,
  onChange,
  onReset,
  swatches,
  invalid = false,
  extra,
}: {
  label: string;
  hint?: string;
  value: string;
  overridden: boolean;
  onChange: (v: string) => void;
  onReset: () => void;
  swatches?: readonly string[];
  invalid?: boolean;
  extra?: ReactNode;
}) {
  const { t } = useT("theme");
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const onDown = (e: globalThis.PointerEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("pointerdown", onDown);
    return () => window.removeEventListener("pointerdown", onDown);
  }, [open]);
  return (
    <div
      ref={rootRef}
      className={clsx("th-color", open && "th-color--open", invalid && "th-color--invalid")}
      onKeyDown={(e) => {
        if (e.key === "Escape" && open) {
          e.stopPropagation();
          setOpen(false);
        }
      }}
    >
      <div className="th-color__row">
        <button
          type="button"
          className="th-color__swatch"
          aria-expanded={open}
          aria-label={t("picker.open", { name: label })}
          onClick={() => setOpen((o) => !o)}
        >
          <span style={{ background: value }} />
        </button>
        <div className="th-color__text">
          <span className="th-color__label">
            {label}
            {overridden && <span className="th-color__dot" title={t("editor.overridden")} />}
          </span>
          {hint && <span className="th-color__hint">{hint}</span>}
        </div>
        <code className="th-color__value">{value}</code>
        {extra}
        <button
          type="button"
          className="th-color__reset"
          disabled={!overridden}
          aria-label={t("editor.resetToken", { name: label })}
          title={t("editor.resetToken", { name: label })}
          onClick={onReset}
        >
          <RotateCcw size={12} />
        </button>
      </div>
      {open && <ColorPicker value={value} onChange={onChange} swatches={swatches} />}
    </div>
  );
}
