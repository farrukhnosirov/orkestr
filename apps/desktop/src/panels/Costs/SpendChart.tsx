import type { CostPoint } from "@orkestr/protocol";
import { useState } from "react";

import { dayLabel, formatUsd, niceMax } from "./format";

const W = 640;
const H = 180;
const PAD = { top: 12, right: 8, bottom: 22, left: 52 };
const GAP = 2;

export interface BarGeometry {
  x: number;
  width: number;
  reportedY: number;
  reportedH: number;
  estimatedY: number;
  estimatedH: number;
}

/** Pure layout of stacked bars (reported at the baseline, estimated on top). */
export function layoutBars(points: CostPoint[], max: number): BarGeometry[] {
  const plotW = W - PAD.left - PAD.right;
  const plotH = H - PAD.top - PAD.bottom;
  const slot = points.length > 0 ? plotW / points.length : plotW;
  const width = Math.max(2, slot - Math.max(2, slot * 0.28));
  return points.map((p, i) => {
    const est = Math.max(0, p.totals.estimatedCostUsd);
    const rep = Math.max(0, p.totals.costUsd - est);
    const repH = (rep / max) * plotH;
    const estH = (est / max) * plotH;
    const base = PAD.top + plotH;
    const gap = repH > 0 && estH > 0 ? GAP : 0;
    return {
      x: PAD.left + i * slot + (slot - width) / 2,
      width,
      reportedY: base - repH,
      reportedH: repH,
      estimatedY: base - repH - estH,
      estimatedH: Math.max(0, estH - gap),
    };
  });
}

/** Path for a bar segment with 4px rounded top corners (flat at the bottom). */
function topRounded(x: number, y: number, w: number, h: number): string {
  if (h <= 0) return "";
  const r = Math.min(4, w / 2, h);
  return `M${x},${y + h}V${y + r}Q${x},${y} ${x + r},${y}H${x + w - r}Q${x + w},${y} ${x + w},${y + r}V${y + h}Z`;
}

/** Daily spend bar chart: reported vs estimated cost (stacked), with hover/focus tooltip. */
export function SpendChart({ points }: { points: CostPoint[] }) {
  const [active, setActive] = useState<number | null>(null);
  const maxValue = niceMax(Math.max(0, ...points.map((p) => p.totals.costUsd)));
  const bars = layoutBars(points, maxValue);
  const plotH = H - PAD.top - PAD.bottom;
  const ticks = [0, 0.5, 1].map((f) => ({ v: maxValue * f, y: PAD.top + plotH - plotH * f }));
  const total = points.reduce((s, p) => s + p.totals.costUsd, 0);
  const labelEvery = Math.max(1, Math.ceil(points.length / 6));
  const activePoint = active !== null ? points[active] : undefined;

  return (
    <figure className="cost-chart">
      <div className="cost-chart__legend" aria-hidden="true">
        <span>
          <i className="cost-swatch cost-swatch--reported" /> Reported
        </span>
        <span>
          <i className="cost-swatch cost-swatch--estimated" /> Estimated
        </span>
        <span className="cost-chart__readout">
          {activePoint
            ? `${dayLabel(activePoint.bucket)} · ${formatUsd(activePoint.totals.costUsd)} (${formatUsd(activePoint.totals.estimatedCostUsd)} est.) · ${activePoint.totals.requests} calls`
            : `${points.length} days · ${formatUsd(total)}`}
        </span>
      </div>
      <svg
        viewBox={`0 0 ${W} ${H}`}
        className="cost-chart__svg"
        role="img"
        aria-label={`Daily spend over the last ${points.length} days, total ${formatUsd(total)}`}
        onMouseLeave={() => setActive(null)}
      >
        {ticks.map((t) => (
          <g key={t.v}>
            <line x1={PAD.left} x2={W - PAD.right} y1={t.y} y2={t.y} className="cost-chart__grid" />
            <text x={PAD.left - 6} y={t.y + 3} className="cost-chart__axis" textAnchor="end">
              {formatUsd(t.v)}
            </text>
          </g>
        ))}
        {bars.map((b, i) => {
          const p = points[i];
          if (!p) return null;
          const slotX = b.x - 2;
          return (
            <g
              key={p.bucket}
              tabIndex={0}
              className="cost-chart__bar"
              aria-label={`${dayLabel(p.bucket)}: ${formatUsd(p.totals.costUsd)}, of which ${formatUsd(p.totals.estimatedCostUsd)} estimated`}
              onMouseEnter={() => setActive(i)}
              onFocus={() => setActive(i)}
              onBlur={() => setActive(null)}
            >
              <rect x={slotX} y={PAD.top} width={b.width + 4} height={plotH} className="cost-chart__hit" data-active={active === i || undefined} />
              <path
                d={b.estimatedH > 0 && b.reportedH > 0 ? `M${b.x},${b.reportedY}h${b.width}v${b.reportedH}h${-b.width}Z` : topRounded(b.x, b.reportedY, b.width, b.reportedH)}
                className="cost-chart__reported"
              />
              <path d={topRounded(b.x, b.estimatedY, b.width, b.estimatedH)} className="cost-chart__estimated" />
              {i % labelEvery === 0 && (
                <text x={b.x + b.width / 2} y={H - 6} className="cost-chart__axis" textAnchor="middle">
                  {dayLabel(p.bucket)}
                </text>
              )}
            </g>
          );
        })}
      </svg>
    </figure>
  );
}
