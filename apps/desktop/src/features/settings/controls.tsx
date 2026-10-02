/**
 * Small form controls for settings and onboarding, styled with `--ok-*`
 * tokens (see settings.css). Keyboard accessible: Switch is a native button
 * with `role="switch"`, Segmented is a radio group with arrow-key navigation.
 */
import { Spinner } from "@orkestr/ui";
import { clsx } from "clsx";
import { type KeyboardEvent, type ReactNode, useId, useRef } from "react";

import { formatBytes as i18nFormatBytes, formatDuration as i18nFormatDuration, useT } from "../../i18n";

export interface SwitchProps {
  checked: boolean;
  onChange: (next: boolean) => void;
  label: string;
  disabled?: boolean;
  busy?: boolean;
}

export function Switch({ checked, onChange, label, disabled = false, busy = false }: SwitchProps) {
  const { t } = useT("common");
  return (
    <span className="ok-switch-wrap">
      {busy && <Spinner size={12} label={t("state.saving")} />}
      <button
        type="button"
        role="switch"
        aria-checked={checked}
        aria-label={label}
        disabled={disabled || busy}
        className={clsx("ok-switch", checked && "ok-switch--on")}
        onClick={() => onChange(!checked)}
      >
        <span className="ok-switch__thumb" />
      </button>
    </span>
  );
}

export interface SegmentedOption<T extends string> {
  value: T;
  label: ReactNode;
  disabled?: boolean;
  title?: string;
}

export interface SegmentedProps<T extends string> {
  value: T;
  options: readonly SegmentedOption<T>[];
  onChange: (v: T) => void;
  label: string;
  disabled?: boolean;
}

export function Segmented<T extends string>({ value, options, onChange, label, disabled = false }: SegmentedProps<T>) {
  const refs = useRef<Array<HTMLButtonElement | null>>([]);
  const enabled = options.map((o, i) => (o.disabled ? -1 : i)).filter((i) => i >= 0);
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const pos = enabled.indexOf(options.findIndex((o) => o.value === value));
    let next: number | undefined;
    if (e.key === "ArrowRight" || e.key === "ArrowDown") next = enabled[(pos + 1) % enabled.length];
    else if (e.key === "ArrowLeft" || e.key === "ArrowUp") next = enabled[(pos - 1 + enabled.length) % enabled.length];
    if (next === undefined) return;
    e.preventDefault();
    const opt = options[next];
    if (opt) {
      onChange(opt.value);
      refs.current[next]?.focus();
    }
  };
  return (
    <div role="radiogroup" aria-label={label} className="ok-segmented" onKeyDown={onKeyDown}>
      {options.map((o, i) => {
        const selected = o.value === value;
        return (
          <button
            key={o.value}
            ref={(el) => {
              refs.current[i] = el;
            }}
            type="button"
            role="radio"
            aria-checked={selected}
            tabIndex={selected ? 0 : -1}
            title={o.title}
            disabled={disabled || o.disabled}
            className={clsx("ok-segmented__item", selected && "ok-segmented__item--active")}
            onClick={() => onChange(o.value)}
          >
            {o.label}
          </button>
        );
      })}
    </div>
  );
}

export interface SettingRowProps {
  title: ReactNode;
  description?: ReactNode;
  children: ReactNode;
  /** Stack the control under the text (wide controls). */
  stacked?: boolean;
}

export function SettingRow({ title, description, children, stacked = false }: SettingRowProps) {
  const id = useId();
  return (
    <div className={clsx("set-row", stacked && "set-row--stacked")} role="group" aria-labelledby={id}>
      <div className="set-row__text">
        <div className="set-row__title" id={id}>
          {title}
        </div>
        {description != null && <div className="set-row__desc">{description}</div>}
      </div>
      <div className="set-row__control">{children}</div>
    </div>
  );
}

export function SettingGroup({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="set-group" aria-label={title}>
      <h3 className="set-group__title">{title}</h3>
      <div className="set-group__body">{children}</div>
    </section>
  );
}

/** Localized byte sizes (binary units); "—" for missing values. */
export function formatBytes(bytes: number | null | undefined): string {
  if (bytes == null || !Number.isFinite(bytes) || bytes < 0) return "—";
  return i18nFormatBytes(bytes);
}

/** Localized durations (`3 h 12 min` / `3 ч 12 мин`). */
export function formatDuration(ms: number): string {
  return i18nFormatDuration(ms);
}
