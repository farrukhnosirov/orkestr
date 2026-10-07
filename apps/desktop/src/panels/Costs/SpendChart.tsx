import type { CostPoint } from "@orkestr/protocol";
import { type RefObject, useEffect, useRef, useState } from "react";

import { useLanguage, useT } from "../../i18n";
import { dayLabel, formatUsd, niceMax } from "./format";

/** Default / fallback drawing width; the chart redraws at its measured pixel width. */
const W = 640;
const H = 180;
/** Narrowest width the chart lays itself out for (below it the SVG scales down). */
const MIN_W = 280;
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
export function layoutBars(points: CostPoint[], max: number, width = W): BarGeometry[] {
  const plotW = width - PAD.left - PAD.right;
  const plotH = H - PAD.top - PAD.bottom;
  const slot = points.length > 0 ? plotW / points.length : plotW;
  const barW = Math.min(48, Math.max(2, slot - Math.max(2, slot * 0.28)));
  return points.map((p, i) => {
    const est = Math.max(0, p.totals.estimatedCostUsd);
    const rep = Math.max(0, p.totals.costUsd - est);
    const repH = (rep / max) * plotH;
    const estH = (est / max) * plotH;
    const base = PAD.top + plotH;
    const gap = repH > 0 && estH > 0 ? GAP : 0;
    return {
      x: PAD.left + i * slot + (slot - barW) / 2,
      width: barW,
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

/**
 * Drawing width of the chart: its measured pixel width (so one SVG unit is
 * one CSS pixel and the 10px axis text stays 10px at any panel width).
 */
export function chartWidth(measured: number | null | undefined): number {
  if (!measured || !Number.isFinite(measured) || measured <= 0) return W;
  return Math.max(MIN_W, Math.round(measured));
}

function useMeasuredWidth(): [RefObject<HTMLElement>, number | null] {
  const ref = useRef<HTMLElement>(null);
  const [width, setWidth] = useState<number | null>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el) return undefined;
    setWidth(el.clientWidth || null);
    if (typeof ResizeObserver === "undefined") return undefined;
    const ro = new ResizeObserver((entries) => {
      const w = entries[0]?.contentRect.width;
      if (w) setWidth(w);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return [ref, width];
}

/** Daily spend bar chart: reported vs estimated cost (stacked), with hover/focus tooltip. */
export function SpendChart({ points }: { points: CostPoint[] }) {
  const { t } = useT("costs");
  const lng = useLanguage();
  const usd = (v: number) => formatUsd(v, lng);
  const day = (key: string) => dayLabel(key, lng);
  const [active, setActive] = useState<number | null>(null);
  const [figureRef, measured] = useMeasuredWidth();
  const w = chartWidth(measured);
  const maxValue = niceMax(Math.max(0, ...points.map((p) => p.totals.costUsd)));
  const bars = layoutBars(points, maxValue, w);
  const plotH = H - PAD.top - PAD.bottom;
  const ticks = [0, 0.5, 1].map((f) => ({ v: maxValue * f, y: PAD.top + plotH - plotH * f }));
  const total = points.reduce((s, p) => s + p.totals.costUsd, 0);
  // About one day label per 90px so wide charts get more labels, narrow ones fewer.
  const labelEvery = Math.max(1, Math.ceil(points.length / Math.max(3, Math.floor(w / 90))));
  const activePoint = active !== null ? points[active] : undefined;

  return (
    <figure className="cost-chart" ref={figureRef}>
      <div className="cost-chart__legend" aria-hidden="true">
        <span>
          <i className="cost-swatch cost-swatch--reported" /> {t("chart.reported")}
        </span>
        <span>
          <i className="cost-swatch cost-swatch--estimated" /> {t("chart.estimated")}
        </span>
        <span className="cost-chart__readout">
          {activePoint
            ? t("chart.readout", {
                day: day(activePoint.bucket),
                cost: usd(activePoint.totals.costUsd),
                estimated: usd(activePoint.totals.estimatedCostUsd),
                count: activePoint.totals.requests,
              })
            : t("chart.total", { count: points.length, total: usd(total) })}
        </span>
      </div>
      <svg
        viewBox={`0 0 ${w} ${H}`}
        className="cost-chart__svg"
        role="img"
        aria-label={t("chart.aria", { count: points.length, total: usd(total) })}
        onMouseLeave={() => setActive(null)}
      >
        {ticks.map((tick) => (
          <g key={tick.v}>
            <line x1={PAD.left} x2={w - PAD.right} y1={tick.y} y2={tick.y} className="cost-chart__grid" />
            <text x={PAD.left - 6} y={tick.y + 3} className="cost-chart__axis" textAnchor="end">
              {usd(tick.v)}
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
              aria-label={t("chart.bar", { day: day(p.bucket), cost: usd(p.totals.costUsd), estimated: usd(p.totals.estimatedCostUsd) })}
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
                  {day(p.bucket)}
                </text>
              )}
            </g>
          );
        })}
      </svg>
    </figure>
  );
}
