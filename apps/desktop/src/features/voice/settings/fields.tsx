/** Small custom controls for voice/sound settings (tokens only, keyboard accessible). */
import { clsx } from "clsx";
import { type ReactNode, useEffect, useState } from "react";

export interface SliderProps {
  value: number;
  min: number;
  max: number;
  step?: number;
  label: string;
  /** Commit (on release / keyboard). */
  onCommit: (v: number) => void;
  format?: (v: number) => string;
  disabled?: boolean;
}

/** Range slider that previews locally and commits on release (one RPC per gesture). */
export function Slider({ value, min, max, step = 1, label, onCommit, format, disabled = false }: SliderProps) {
  const [local, setLocal] = useState(value);
  useEffect(() => setLocal(value), [value]);
  const pct = ((local - min) / Math.max(1, max - min)) * 100;
  const commit = () => {
    if (local !== value) onCommit(local);
  };
  return (
    <span className={clsx("vx-slider", disabled && "vx-slider--disabled")}>
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={local}
        disabled={disabled}
        aria-label={label}
        aria-valuetext={format ? format(local) : String(local)}
        style={{ ["--vx-pct" as string]: `${pct}%` }}
        onChange={(e) => setLocal(Number(e.target.value))}
        onPointerUp={commit}
        onKeyUp={commit}
        onBlur={commit}
      />
      <span className="vx-slider__value">{format ? format(local) : local}</span>
    </span>
  );
}

export interface SelectOption {
  value: string;
  label: string;
  disabled?: boolean;
}

export function Select({
  value,
  options,
  onChange,
  label,
  disabled = false,
}: {
  value: string;
  options: readonly SelectOption[];
  onChange: (v: string) => void;
  label: string;
  disabled?: boolean;
}) {
  return (
    <select className="set-select vx-select" value={value} aria-label={label} disabled={disabled} onChange={(e) => onChange(e.target.value)}>
      {options.map((o) => (
        <option key={o.value} value={o.value} disabled={o.disabled}>
          {o.label}
        </option>
      ))}
    </select>
  );
}

/** Highlighted explanation card (consent, privacy notes). */
export function Notice({ tone = "info", icon, title, children, actions }: { tone?: "info" | "warning" | "ok"; icon?: ReactNode; title: string; children?: ReactNode; actions?: ReactNode }) {
  return (
    <div className={clsx("vx-notice", `vx-notice--${tone}`)} role={tone === "warning" ? "alert" : "note"}>
      {icon && <span className="vx-notice__icon">{icon}</span>}
      <div className="vx-notice__body">
        <div className="vx-notice__title">{title}</div>
        {children && <div className="vx-notice__text">{children}</div>}
        {actions && <div className="vx-notice__actions">{actions}</div>}
      </div>
    </div>
  );
}
