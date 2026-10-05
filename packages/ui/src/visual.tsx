/**
 * Visual OS primitives (§28, Phase 3): skeleton loaders and procedural
 * empty-state illustrations. Everything is SVG/CSS drawn from `--ok-*`
 * tokens (no binary assets); motion is CSS and stops under
 * `data-ok-motion="off"` / `prefers-reduced-motion`.
 */
import { clsx } from "clsx";
import { useId, type CSSProperties, type HTMLAttributes } from "react";

// ── Skeleton ─────────────────────────────────────────────

export interface SkeletonProps extends HTMLAttributes<HTMLSpanElement> {
  width?: number | string;
  height?: number | string;
  /** Pill / circle / default radius. */
  shape?: "rect" | "pill" | "circle";
}

/** A shimmering placeholder block (decorative; pair it with an `aria-busy` container). */
export function Skeleton({ width = "100%", height = 12, shape = "rect", className, style, ...rest }: SkeletonProps) {
  const s: CSSProperties = { width, height, ...style };
  return <span aria-hidden="true" className={clsx("ok-skeleton", `ok-skeleton--${shape}`, className)} style={s} {...rest} />;
}

export interface SkeletonLinesProps extends HTMLAttributes<HTMLDivElement> {
  lines?: number;
  /** Accessible label announced while loading. */
  label?: string;
}

/** A paragraph-shaped skeleton (last line shorter). */
export function SkeletonLines({ lines = 3, label, className, ...rest }: SkeletonLinesProps) {
  return (
    <div role="status" aria-busy="true" aria-label={label} className={clsx("ok-skeleton-lines", className)} {...rest}>
      {Array.from({ length: Math.max(1, lines) }, (_, i) => (
        <Skeleton key={i} width={i === lines - 1 ? "62%" : `${92 - (i % 3) * 6}%`} />
      ))}
    </div>
  );
}

// ── Illustrations ────────────────────────────────────────

export type IllustrationKind = "orbit" | "constellation" | "inbox" | "signal";

export interface IllustrationProps {
  kind: IllustrationKind;
  size?: number;
  className?: string;
}

/**
 * Procedural empty-state art: a core with orbits (nothing running yet), a
 * constellation (no items), an open tray (nothing waiting) or a signal
 * (no data yet).
 */
export function Illustration({ kind, size = 120, className }: IllustrationProps) {
  const id = useId().replace(/:/g, "");
  const grad = `${id}-g`;
  const glow = `${id}-glow`;
  return (
    <svg className={clsx("ok-illustration", `ok-illustration--${kind}`, className)} width={size} height={size} viewBox="0 0 120 120" aria-hidden="true">
      <defs>
        <linearGradient id={grad} x1="0" y1="0" x2="1" y2="1">
          <stop offset="0%" stopColor="var(--ok-color-accent)" />
          <stop offset="100%" stopColor="var(--ok-color-violet)" />
        </linearGradient>
        <radialGradient id={glow} cx="50%" cy="50%" r="50%">
          <stop offset="0%" stopColor="var(--ok-color-accent)" stopOpacity="0.35" />
          <stop offset="100%" stopColor="var(--ok-color-accent)" stopOpacity="0" />
        </radialGradient>
      </defs>
      <circle cx="60" cy="60" r="56" fill={`url(#${glow})`} />
      {kind === "orbit" && (
        <g>
          <ellipse className="ok-illustration__orbit" cx="60" cy="60" rx="46" ry="18" fill="none" stroke={`url(#${grad})`} strokeWidth="1.5" transform="rotate(-24 60 60)" />
          <ellipse className="ok-illustration__orbit ok-illustration__orbit--slow" cx="60" cy="60" rx="34" ry="12" fill="none" stroke="var(--ok-color-border-strong)" strokeWidth="1" strokeDasharray="3 5" transform="rotate(30 60 60)" />
          <circle cx="60" cy="60" r="11" fill={`url(#${grad})`} />
          <circle cx="60" cy="56" r="4" fill="var(--ok-color-text)" opacity="0.6" />
          <g className="ok-illustration__spin">
            <circle cx="102" cy="44" r="3.5" fill="var(--ok-color-violet)" />
          </g>
        </g>
      )}
      {kind === "constellation" && (
        <g>
          <polyline points="22,78 44,52 66,66 88,34 100,58" fill="none" stroke={`url(#${grad})`} strokeWidth="1.4" strokeLinejoin="round" />
          {[
            [22, 78, 3.5],
            [44, 52, 4.5],
            [66, 66, 3],
            [88, 34, 5],
            [100, 58, 3],
          ].map(([x, y, r], i) => (
            <circle key={i} className="ok-illustration__twinkle" style={{ animationDelay: `${i * 0.4}s` }} cx={x} cy={y} r={r} fill={i % 2 ? "var(--ok-color-violet)" : "var(--ok-color-accent)"} />
          ))}
        </g>
      )}
      {kind === "inbox" && (
        <g>
          <path d="M28 64 L40 40 H80 L92 64 V84 a6 6 0 0 1 -6 6 H34 a6 6 0 0 1 -6 -6 Z" fill="var(--ok-color-surface-strong)" stroke={`url(#${grad})`} strokeWidth="1.5" />
          <path d="M28 64 H48 a12 12 0 0 0 24 0 H92" fill="none" stroke={`url(#${grad})`} strokeWidth="1.5" />
          <path className="ok-illustration__float" d="M60 22 v18 m-7 -7 l7 7 l7 -7" fill="none" stroke="var(--ok-color-accent)" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" />
        </g>
      )}
      {kind === "signal" && (
        <g>
          {[18, 30, 42].map((r, i) => (
            <path
              key={r}
              className="ok-illustration__wave"
              style={{ animationDelay: `${i * 0.35}s` }}
              d={`M${60 - r} ${72 - r * 0.15} a${r} ${r} 0 0 1 ${r * 2} 0`}
              fill="none"
              stroke={`url(#${grad})`}
              strokeWidth="2"
              strokeLinecap="round"
            />
          ))}
          <circle cx="60" cy="76" r="5" fill={`url(#${grad})`} />
        </g>
      )}
    </svg>
  );
}
