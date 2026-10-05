/**
 * Theme gallery card: a miniature, *live-styled* app window. The card scopes
 * the theme's tokens on its own element (`data-ok-theme`/`data-ok-mode` +
 * inline `--ok-*`), so every card renders its own theme without touching
 * `:root`.
 */
import type { ContrastCheck, ThemeConfig } from "@orkestr/protocol";
import { contrastReport } from "@orkestr/ui";
import { clsx } from "clsx";
import { Check, Copy, Download, Pencil, Trash2 } from "lucide-react";
import type { CSSProperties, ReactNode } from "react";

import { useT } from "../../i18n";
import { themeDescription } from "./i18nText";

/** Inline style that scopes a theme's tokens on one element. */
export function scopedThemeStyle(theme: ThemeConfig): CSSProperties {
  const style: Record<string, string> = {};
  for (const [k, v] of Object.entries(theme.tokens)) style[`--ok-${k}`] = v;
  style["--ok-glow-intensity"] = String(theme.effects.glowIntensity);
  style["--ok-blur"] = `${Math.min(theme.effects.blurPx, 12)}px`;
  return style as CSSProperties;
}

/** Scope attributes for an element that should render `theme`. */
export function scopedThemeAttrs(theme: ThemeConfig): Record<string, string> {
  return { "data-ok-theme": theme.id, "data-ok-mode": theme.mode === "Light" ? "light" : "dark" };
}

/** The miniature window (also used by the editor's preview strip). */
export function ThemeMiniature({ theme, className }: { theme: ThemeConfig; className?: string }) {
  return (
    <div className={clsx("th-mini", className)} {...scopedThemeAttrs(theme)} style={scopedThemeStyle(theme)} aria-hidden="true">
      <div className={clsx("th-mini__bg", `th-mini__bg--${theme.background.kind.toLowerCase()}`)} />
      <div className="th-mini__bar">
        <span className="th-mini__logo" />
        <span className="th-mini__line th-mini__line--short" />
        <span className="th-mini__stop" />
      </div>
      <div className="th-mini__body">
        <div className="th-mini__rail">
          <span />
          <span />
          <span />
        </div>
        <div className="th-mini__panel">
          <span className="th-mini__line" />
          <span className="th-mini__line th-mini__line--muted" />
          <span className="th-mini__line th-mini__line--muted th-mini__line--short" />
          <div className="th-mini__chips">
            <span className="th-mini__chip th-mini__chip--success" />
            <span className="th-mini__chip th-mini__chip--warning" />
            <span className="th-mini__chip th-mini__chip--danger" />
          </div>
          <span className="th-mini__button" />
        </div>
        <div className="th-mini__panel th-mini__panel--side">
          <span className="th-mini__line th-mini__line--muted" />
          <span className="th-mini__ring" />
        </div>
      </div>
    </div>
  );
}

/** Worst contrast outcome: `fail` (hard), `warn` (soft), `pass`. */
export function contrastLevel(checks: readonly ContrastCheck[]): "pass" | "warn" | "fail" {
  if (checks.some((c) => c.hard && !c.pass)) return "fail";
  if (checks.some((c) => !c.pass)) return "warn";
  return "pass";
}

export interface ThemeCardProps {
  theme: ThemeConfig;
  builtin: boolean;
  active: boolean;
  busy?: boolean;
  onSelect: () => void;
  onEdit: () => void;
  onDuplicate: () => void;
  onExport: () => void;
  onDelete?: () => void;
}

function CardAction({ label, icon, onClick, danger = false }: { label: string; icon: ReactNode; onClick: () => void; danger?: boolean }) {
  return (
    <button
      type="button"
      className={clsx("th-card__action", danger && "th-card__action--danger")}
      aria-label={label}
      title={label}
      onClick={(e) => {
        e.stopPropagation();
        onClick();
      }}
    >
      {icon}
    </button>
  );
}

export function ThemeCard({ theme, builtin, active, busy = false, onSelect, onEdit, onDuplicate, onExport, onDelete }: ThemeCardProps) {
  const { t } = useT("theme");
  const level = contrastLevel(contrastReport(theme));
  return (
    <div className={clsx("th-card", active && "th-card--active")}>
      <button
        type="button"
        role="radio"
        aria-checked={active}
        className="th-card__main"
        disabled={busy}
        onClick={onSelect}
        aria-label={t("gallery.select", { name: theme.name })}
      >
        <ThemeMiniature theme={theme} />
        <span className="th-card__meta">
          <span className="th-card__name">
            {theme.name}
            {active && <Check size={13} className="th-card__check" aria-hidden="true" />}
          </span>
          <span className="th-card__desc">{themeDescription(theme, builtin)}</span>
          <span className="th-card__tags">
            <span className="th-tag">{t(`modes.${theme.mode}`)}</span>
            <span className="th-tag">{builtin ? t("gallery.builtin") : t("gallery.custom")}</span>
            <span className={clsx("th-tag", `th-tag--${level}`)}>{t(`contrast.level.${level}`)}</span>
          </span>
        </span>
      </button>
      <div className="th-card__actions">
        <CardAction label={t("gallery.customize")} icon={<Pencil size={13} />} onClick={onEdit} />
        <CardAction label={t("gallery.duplicate")} icon={<Copy size={13} />} onClick={onDuplicate} />
        <CardAction label={t("gallery.export")} icon={<Download size={13} />} onClick={onExport} />
        {onDelete && <CardAction label={t("gallery.delete")} icon={<Trash2 size={13} />} onClick={onDelete} danger />}
      </div>
    </div>
  );
}
