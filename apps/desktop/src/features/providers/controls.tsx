/**
 * Small form controls for the provider settings, built on @orkestr/ui tokens.
 */
import { clsx } from "clsx";
import { type ReactNode, type SelectHTMLAttributes, type TextareaHTMLAttributes, useId } from "react";

export interface SwitchProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: ReactNode;
  disabled?: boolean;
}

/** Accessible toggle (`role="switch"`, Space/Enter toggles). */
export function Switch({ checked, onChange, label, disabled }: SwitchProps) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      className="pv-switch"
      disabled={disabled}
      onClick={() => onChange(!checked)}
    >
      <span className="pv-switch__track" aria-hidden="true" />
      <span>{label}</span>
    </button>
  );
}

export interface SelectProps extends SelectHTMLAttributes<HTMLSelectElement> {
  label: ReactNode;
  hint?: ReactNode;
  children: ReactNode;
}

/** Labelled native select (keyboard + screen-reader friendly). */
export function Select({ label, hint, className, id, children, ...rest }: SelectProps) {
  const auto = useId();
  const selectId = id ?? auto;
  return (
    <div className={clsx("ok-field", className)}>
      <label className="ok-field__label" htmlFor={selectId}>
        {label}
      </label>
      <select id={selectId} className="pv-select" aria-describedby={hint ? `${selectId}-hint` : undefined} {...rest}>
        {children}
      </select>
      {hint != null && (
        <div id={`${selectId}-hint`} className="ok-field__hint">
          {hint}
        </div>
      )}
    </div>
  );
}

export interface TextAreaProps extends TextareaHTMLAttributes<HTMLTextAreaElement> {
  label: ReactNode;
  hint?: ReactNode;
}

/** Labelled textarea. */
export function TextArea({ label, hint, className, id, ...rest }: TextAreaProps) {
  const auto = useId();
  const areaId = id ?? auto;
  return (
    <div className={clsx("ok-field", className)}>
      <label className="ok-field__label" htmlFor={areaId}>
        {label}
      </label>
      <textarea id={areaId} className="pv-textarea" aria-describedby={hint ? `${areaId}-hint` : undefined} {...rest} />
      {hint != null && (
        <div id={`${areaId}-hint`} className="ok-field__hint">
          {hint}
        </div>
      )}
    </div>
  );
}
