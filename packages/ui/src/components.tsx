/**
 * Orkestr primitives. Styling lives in `styles/components.css` and only uses
 * `--ok-*` tokens, so every primitive follows the active theme.
 */
import { clsx } from "clsx";
import {
  type ButtonHTMLAttributes,
  type HTMLAttributes,
  type InputHTMLAttributes,
  type KeyboardEvent,
  type ReactNode,
  forwardRef,
  useId,
  useRef,
} from "react";

export type Size = "sm" | "md" | "lg";

// ── Spinner ───────────────────────────────────────────────

export interface SpinnerProps extends HTMLAttributes<HTMLSpanElement> {
  size?: number;
  label?: string;
}

export function Spinner({ size = 16, label = "Loading", className, style, ...rest }: SpinnerProps) {
  return (
    <span
      role="status"
      aria-label={label}
      className={clsx("ok-spinner", className)}
      style={{ ["--_s" as string]: `${size}px`, ...style }}
      {...rest}
    />
  );
}

// ── Button ────────────────────────────────────────────────

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: "primary" | "secondary" | "ghost" | "danger";
  size?: Size;
  loading?: boolean;
  leftIcon?: ReactNode;
  rightIcon?: ReactNode;
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  { variant = "secondary", size = "md", loading = false, leftIcon, rightIcon, className, children, disabled, type, ...rest },
  ref,
) {
  return (
    <button
      ref={ref}
      type={type ?? "button"}
      className={clsx("ok-btn", `ok-btn--${variant}`, size !== "md" && `ok-btn--${size}`, className)}
      disabled={disabled || loading}
      aria-busy={loading || undefined}
      data-loading={loading || undefined}
      {...rest}
    >
      {leftIcon && <span className="ok-btn__icon">{leftIcon}</span>}
      {children != null && <span className="ok-btn__label">{children}</span>}
      {rightIcon && <span className="ok-btn__icon">{rightIcon}</span>}
      {loading && (
        <span className="ok-btn__spinner">
          <Spinner size={14} />
        </span>
      )}
    </button>
  );
});

// ── IconButton ────────────────────────────────────────────

export interface IconButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  /** Accessible label (also used as tooltip text when `tooltip` is true). */
  label: string;
  icon: ReactNode;
  size?: Size;
  variant?: "ghost" | "solid" | "danger";
  active?: boolean;
  tooltip?: boolean | "top" | "bottom" | "left" | "right";
}

export const IconButton = forwardRef<HTMLButtonElement, IconButtonProps>(function IconButton(
  { label, icon, size = "md", variant = "ghost", active = false, tooltip = false, className, type, ...rest },
  ref,
) {
  const button = (
    <button
      ref={ref}
      type={type ?? "button"}
      aria-label={label}
      className={clsx(
        "ok-icon-btn",
        size !== "md" && `ok-icon-btn--${size}`,
        variant !== "ghost" && `ok-icon-btn--${variant}`,
        active && "ok-icon-btn--active",
        className,
      )}
      {...rest}
    >
      {icon}
    </button>
  );
  if (!tooltip) return button;
  return (
    <Tooltip content={label} side={tooltip === true ? "bottom" : tooltip}>
      {button}
    </Tooltip>
  );
});

// ── Panel / GlassPanel ────────────────────────────────────

export interface PanelProps extends Omit<HTMLAttributes<HTMLElement>, "title"> {
  title?: ReactNode;
  actions?: ReactNode;
  padded?: boolean;
  glass?: boolean;
  glow?: boolean;
}

export function Panel({ title, actions, padded = true, glass = false, glow = false, className, children, ...rest }: PanelProps) {
  return (
    <section className={clsx("ok-panel", glass && "ok-panel--glass", glow && "ok-panel--glow", className)} {...rest}>
      {(title != null || actions != null) && (
        <header className="ok-panel__header">
          <div className="ok-panel__title">{title}</div>
          {actions != null && <div className="ok-panel__actions">{actions}</div>}
        </header>
      )}
      <div className={clsx("ok-panel__body", padded && "ok-panel__body--padded")}>{children}</div>
    </section>
  );
}

export function GlassPanel(props: Omit<PanelProps, "glass">) {
  return <Panel {...props} glass />;
}

// ── Badge ─────────────────────────────────────────────────

export type Tone = "neutral" | "accent" | "violet" | "success" | "warning" | "danger" | "info";

export interface BadgeProps extends HTMLAttributes<HTMLSpanElement> {
  tone?: Tone;
  icon?: ReactNode;
}

export function Badge({ tone = "neutral", icon, className, children, ...rest }: BadgeProps) {
  return (
    <span className={clsx("ok-badge", tone !== "neutral" && `ok-badge--${tone}`, className)} {...rest}>
      {icon}
      {children}
    </span>
  );
}

// ── Input ─────────────────────────────────────────────────

export interface InputProps extends Omit<InputHTMLAttributes<HTMLInputElement>, "size"> {
  label?: ReactNode;
  hint?: ReactNode;
  error?: ReactNode;
  leftIcon?: ReactNode;
  rightSlot?: ReactNode;
}

export const Input = forwardRef<HTMLInputElement, InputProps>(function Input(
  { label, hint, error, leftIcon, rightSlot, className, id, ...rest },
  ref,
) {
  const autoId = useId();
  const inputId = id ?? autoId;
  const describedBy = error ? `${inputId}-error` : hint ? `${inputId}-hint` : undefined;
  return (
    <div className={clsx("ok-field", className)}>
      {label != null && (
        <label className="ok-field__label" htmlFor={inputId}>
          {label}
        </label>
      )}
      <div className="ok-input" data-invalid={error ? "true" : undefined}>
        {leftIcon}
        <input
          ref={ref}
          id={inputId}
          className="ok-input__control"
          aria-invalid={error ? true : undefined}
          aria-describedby={describedBy}
          {...rest}
        />
        {rightSlot}
      </div>
      {error ? (
        <div id={`${inputId}-error`} className="ok-field__error" role="alert">
          {error}
        </div>
      ) : hint ? (
        <div id={`${inputId}-hint`} className="ok-field__hint">
          {hint}
        </div>
      ) : null}
    </div>
  );
});

// ── Tabs ──────────────────────────────────────────────────

export interface TabItem<T extends string = string> {
  id: T;
  label: ReactNode;
  icon?: ReactNode;
  disabled?: boolean;
}

export interface TabsProps<T extends string = string> {
  items: readonly TabItem<T>[];
  value: T;
  onChange: (id: T) => void;
  "aria-label"?: string;
  className?: string;
}

export function Tabs<T extends string = string>({ items, value, onChange, className, ...aria }: TabsProps<T>) {
  const refs = useRef<Array<HTMLButtonElement | null>>([]);
  const enabled = items.map((it, i) => (it.disabled ? -1 : i)).filter((i) => i >= 0);
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const current = items.findIndex((it) => it.id === value);
    const pos = enabled.indexOf(current);
    let next: number | undefined;
    if (e.key === "ArrowRight") next = enabled[(pos + 1) % enabled.length];
    else if (e.key === "ArrowLeft") next = enabled[(pos - 1 + enabled.length) % enabled.length];
    else if (e.key === "Home") next = enabled[0];
    else if (e.key === "End") next = enabled[enabled.length - 1];
    if (next === undefined) return;
    e.preventDefault();
    const item = items[next];
    if (item) {
      onChange(item.id);
      refs.current[next]?.focus();
    }
  };
  return (
    <div role="tablist" className={clsx("ok-tabs", className)} onKeyDown={onKeyDown} {...aria}>
      {items.map((it, i) => {
        const selected = it.id === value;
        return (
          <button
            key={it.id}
            ref={(el) => {
              refs.current[i] = el;
            }}
            type="button"
            role="tab"
            className="ok-tab"
            aria-selected={selected}
            tabIndex={selected ? 0 : -1}
            disabled={it.disabled}
            onClick={() => onChange(it.id)}
          >
            {it.icon}
            {it.label}
          </button>
        );
      })}
    </div>
  );
}

// ── Tooltip ───────────────────────────────────────────────

export interface TooltipProps {
  content: ReactNode;
  side?: "top" | "bottom" | "left" | "right";
  children: ReactNode;
  className?: string;
}

/** Lightweight CSS tooltip shown on hover and keyboard focus. */
export function Tooltip({ content, side = "top", children, className }: TooltipProps) {
  return (
    <span className={clsx("ok-tooltip", className)}>
      {children}
      <span className="ok-tooltip__bubble" data-side={side} role="tooltip" aria-hidden="true">
        {content}
      </span>
    </span>
  );
}

// ── StatusDot ─────────────────────────────────────────────

export type Status = "online" | "busy" | "connecting" | "warning" | "offline" | "error" | "idle";

export interface StatusDotProps extends HTMLAttributes<HTMLSpanElement> {
  status: Status;
  pulse?: boolean;
  label?: string;
}

export function StatusDot({ status, pulse = false, label, className, ...rest }: StatusDotProps) {
  return (
    <span
      className={clsx("ok-status-dot", `ok-status-dot--${status}`, className)}
      data-pulse={pulse || undefined}
      role={label ? "img" : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
      {...rest}
    />
  );
}

// ── EmptyState ────────────────────────────────────────────

export interface EmptyStateProps extends Omit<HTMLAttributes<HTMLDivElement>, "title"> {
  icon?: ReactNode;
  title: ReactNode;
  description?: ReactNode;
  action?: ReactNode;
  badge?: ReactNode;
}

export function EmptyState({ icon, title, description, action, badge, className, ...rest }: EmptyStateProps) {
  return (
    <div className={clsx("ok-empty", className)} {...rest}>
      {icon != null && <div className="ok-empty__icon">{icon}</div>}
      {badge}
      <h3 className="ok-empty__title">{title}</h3>
      {description != null && <p className="ok-empty__description">{description}</p>}
      {action != null && <div className="ok-empty__action">{action}</div>}
    </div>
  );
}

// ── Kbd ───────────────────────────────────────────────────

export function Kbd({ className, ...rest }: HTMLAttributes<HTMLElement>) {
  return <kbd className={clsx("ok-kbd", className)} {...rest} />;
}
