/**
 * Lightweight SVG charts shared by Mission Control and Monitoring: a
 * sparkline, a time-series area chart with crosshair tooltip, and a radial
 * gauge. Pure SVG (no canvas/WebGL) — cheap to redraw at the 2 s sample
 * rate; gradients/glow only when the visual budget allows (`rich`).
 * One series per chart (the title names it; no dual axes).
 */
import { clsx } from "clsx";
import { useId, useMemo, useRef, useState } from "react";

export type ChartTone = "accent" | "violet" | "info" | "success" | "warning" | "danger";

/** Polyline points for values in [0, max] inside a w×h box (pure, unit-tested). */
export function linePoints(values: readonly number[], w: number, h: number, max: number, pad = 2): Array<[number, number]> {
  if (values.length === 0) return [];
  const top = Math.max(1e-9, max);
  const innerH = h - pad * 2;
  const step = values.length > 1 ? w / (values.length - 1) : 0;
  return values.map((v, i) => {
    const clamped = Math.min(top, Math.max(0, Number.isFinite(v) ? v : 0));
    return [values.length > 1 ? i * step : w, pad + innerH - (clamped / top) * innerH];
  });
}

export function pathOf(points: ReadonlyArray<[number, number]>): string {
  return points.map(([x, y], i) => `${i === 0 ? "M" : "L"}${x.toFixed(1)},${y.toFixed(1)}`).join("");
}

function areaOf(points: ReadonlyArray<[number, number]>, h: number): string {
  const first = points[0];
  const last = points[points.length - 1];
  if (!first || !last) return "";
  return `${pathOf(points)}L${last[0].toFixed(1)},${h}L${first[0].toFixed(1)},${h}Z`;
}

export interface SparklineProps {
  values: readonly number[];
  max?: number;
  tone?: ChartTone;
  rich?: boolean;
  width?: number;
  height?: number;
  /** Accessible summary ("CPU 23 %, last 3 minutes"). */
  label: string;
}

/** Tiny trend line (no axes) with an end marker. */
export function Sparkline({ values, max = 100, tone = "accent", rich = true, width = 120, height = 32, label }: SparklineProps) {
  const gid = useId();
  const pts = useMemo(() => linePoints(values, width, height, max), [values, width, height, max]);
  const last = pts[pts.length - 1];
  return (
    <svg
      className={clsx("ok-chart", `ok-chart--${tone}`)}
      viewBox={`0 0 ${width} ${height}`}
      preserveAspectRatio="none"
      role="img"
      aria-label={label}
      width="100%"
      height={height}
    >
      {rich && (
        <defs>
          <linearGradient id={gid} x1="0" x2="0" y1="0" y2="1">
            <stop offset="0%" className="ok-chart__stop-top" />
            <stop offset="100%" className="ok-chart__stop-bottom" />
          </linearGradient>
        </defs>
      )}
      {pts.length > 1 && rich && <path d={areaOf(pts, height)} fill={`url(#${gid})`} className="ok-chart__area" />}
      {pts.length > 1 && <path d={pathOf(pts)} className="ok-chart__line" vectorEffect="non-scaling-stroke" />}
      {last && <circle cx={last[0]} cy={last[1]} r={2.5} className="ok-chart__dot" vectorEffect="non-scaling-stroke" />}
    </svg>
  );
}

export interface TimeSeriesProps {
  /** Points: time (ms) and value. */
  points: ReadonlyArray<{ ts: number; value: number }>;
  max: number;
  tone?: ChartTone;
  rich?: boolean;
  height?: number;
  label: string;
  /** Formats a value for the axis/tooltip. */
  format: (v: number) => string;
  /** Formats a time for the tooltip. */
  formatTime: (ts: number) => string;
  /** Empty-state text. */
  emptyText: string;
}

const TS_W = 600;

/** Area chart with a recessive grid (0/50/100 % of max) and hover crosshair + tooltip. */
export function TimeSeriesChart({ points, max, tone = "accent", rich = true, height = 140, label, format, formatTime, emptyText }: TimeSeriesProps) {
  const gid = useId();
  const ref = useRef<SVGSVGElement>(null);
  const [hover, setHover] = useState<number | null>(null);
  const values = useMemo(() => points.map((p) => p.value), [points]);
  const pts = useMemo(() => linePoints(values, TS_W, height, max, 6), [values, height, max]);
  if (points.length < 2) {
    return (
      <div className="ok-chart-empty" style={{ height }}>
        {emptyText}
      </div>
    );
  }
  const onMove = (e: React.PointerEvent<SVGSVGElement>) => {
    const box = ref.current?.getBoundingClientRect();
    if (!box || box.width === 0) return;
    const x = ((e.clientX - box.left) / box.width) * TS_W;
    const step = TS_W / (points.length - 1);
    setHover(Math.max(0, Math.min(points.length - 1, Math.round(x / step))));
  };
  const hp = hover !== null ? pts[hover] : undefined;
  const hv = hover !== null ? points[hover] : undefined;
  return (
    <div className="ok-ts">
      <div className="ok-ts__axis" aria-hidden="true">
        <span>{format(max)}</span>
        <span>{format(max / 2)}</span>
        <span>{format(0)}</span>
      </div>
      <div className="ok-ts__plot">
        <svg
          ref={ref}
          className={clsx("ok-chart", "ok-chart--ts", `ok-chart--${tone}`)}
          viewBox={`0 0 ${TS_W} ${height}`}
          preserveAspectRatio="none"
          width="100%"
          height={height}
          role="img"
          aria-label={label}
          onPointerMove={onMove}
          onPointerLeave={() => setHover(null)}
        >
          {rich && (
            <defs>
              <linearGradient id={gid} x1="0" x2="0" y1="0" y2="1">
                <stop offset="0%" className="ok-chart__stop-top" />
                <stop offset="100%" className="ok-chart__stop-bottom" />
              </linearGradient>
            </defs>
          )}
          {[6, height / 2, height - 6].map((y) => (
            <line key={y} x1={0} x2={TS_W} y1={y} y2={y} className="ok-chart__grid" vectorEffect="non-scaling-stroke" />
          ))}
          <path d={areaOf(pts, height)} fill={rich ? `url(#${gid})` : undefined} className={clsx("ok-chart__area", !rich && "ok-chart__area--flat")} />
          <path d={pathOf(pts)} className="ok-chart__line" vectorEffect="non-scaling-stroke" />
          {hp && <line x1={hp[0]} x2={hp[0]} y1={0} y2={height} className="ok-chart__cross" vectorEffect="non-scaling-stroke" />}
        </svg>
        {hp && hv && (
          <div
            className="ok-ts__tip"
            role="status"
            style={{ left: `${(hp[0] / TS_W) * 100}%`, top: `${(hp[1] / height) * 100}%` }}
          >
            <strong>{format(hv.value)}</strong>
            <span>{formatTime(hv.ts)}</span>
          </div>
        )}
        {hp && <span className={clsx("ok-ts__marker", `ok-ts__marker--${tone}`)} style={{ left: `${(hp[0] / TS_W) * 100}%`, top: `${(hp[1] / height) * 100}%` }} />}
      </div>
    </div>
  );
}

export interface GaugeProps {
  /** Fraction 0..1 (null = unknown). */
  value: number | null;
  tone?: ChartTone;
  size?: number;
  /** Center text (already formatted). */
  center: string;
  caption?: string;
  label: string;
  rich?: boolean;
}

/** 270° radial gauge. */
export function Gauge({ value, tone = "accent", size = 92, center, caption, label, rich = true }: GaugeProps) {
  const r = 40;
  const c = 2 * Math.PI * r;
  const arc = c * 0.75;
  const v = value === null ? 0 : Math.min(1, Math.max(0, value));
  return (
    <div className={clsx("ok-gauge", `ok-gauge--${tone}`, rich && "ok-gauge--rich")} style={{ width: size }}>
      <svg viewBox="0 0 100 100" width={size} height={size} role="img" aria-label={label}>
        <circle cx="50" cy="50" r={r} className="ok-gauge__track" strokeDasharray={`${arc} ${c}`} transform="rotate(135 50 50)" />
        {value !== null && v > 0 && (
          <circle cx="50" cy="50" r={r} className="ok-gauge__value" strokeDasharray={`${arc * v} ${c}`} transform="rotate(135 50 50)" />
        )}
      </svg>
      <div className="ok-gauge__center">
        <strong>{center}</strong>
        {caption && <span>{caption}</span>}
      </div>
    </div>
  );
}

/** Horizontal meter bar. */
export function Meter({ value, tone = "accent", label }: { value: number | null; tone?: ChartTone; label: string }) {
  const v = value === null ? 0 : Math.min(1, Math.max(0, value));
  return (
    <div className={clsx("ok-meter", `ok-meter--${tone}`)} role="meter" aria-label={label} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(v * 100)}>
      <span className="ok-meter__fill" style={{ width: `${v * 100}%` }} />
    </div>
  );
}

/** Tone for a utilisation fraction (status thresholds 75 % / 90 %). */
export function loadTone(fraction: number | null, base: ChartTone = "accent"): ChartTone {
  if (fraction === null) return base;
  if (fraction >= 0.9) return "danger";
  if (fraction >= 0.75) return "warning";
  return base;
}
