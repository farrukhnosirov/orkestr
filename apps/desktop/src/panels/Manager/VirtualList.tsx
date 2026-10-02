/**
 * Virtualized vertical list with variable, measured row heights. Only rows
 * intersecting the viewport (plus overscan) are mounted; heights are measured
 * with ResizeObserver and unknown rows use an estimate. Sticks to the bottom
 * while the user is at the bottom (chat behaviour).
 *
 * Without layout information (no ResizeObserver / zero-height viewport, e.g.
 * jsdom or a hidden panel) it renders the last `fallbackCount` rows.
 */
import { type ReactNode, type UIEvent, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

export interface VirtualListProps<T> {
  items: readonly T[];
  getKey: (item: T) => string;
  render: (item: T) => ReactNode;
  estimate?: number;
  overscan?: number;
  fallbackCount?: number;
  className?: string;
  "aria-label"?: string;
  /** Changes whenever content grows in place (streaming) to keep the bottom pinned. */
  tailVersion?: unknown;
}

/** Visible index range for a scroll position (pure, unit-tested). */
export function visibleRange(
  offsets: readonly number[],
  heights: readonly number[],
  scrollTop: number,
  viewport: number,
  overscanPx: number,
): [number, number] {
  const n = heights.length;
  if (n === 0) return [0, -1];
  const top = Math.max(0, scrollTop - overscanPx);
  const bottom = scrollTop + viewport + overscanPx;
  let lo = 0;
  let hi = n - 1;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if ((offsets[mid] ?? 0) + (heights[mid] ?? 0) <= top) lo = mid + 1;
    else hi = mid;
  }
  let end = lo;
  while (end < n - 1 && (offsets[end + 1] ?? 0) < bottom) end += 1;
  return [lo, end];
}

export function VirtualList<T>({
  items,
  getKey,
  render,
  estimate = 96,
  overscan = 600,
  fallbackCount = 60,
  className,
  tailVersion,
  ...aria
}: VirtualListProps<T>) {
  const scroller = useRef<HTMLDivElement>(null);
  const heights = useRef(new Map<string, number>());
  const [measureTick, setMeasureTick] = useState(0);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewport, setViewport] = useState(0);
  const atBottom = useRef(true);
  const observer = useRef<ResizeObserver | null>(null);
  const nodes = useRef(new Map<Element, string>());

  const layoutKnown = viewport > 0 && typeof ResizeObserver !== "undefined";

  useEffect(() => {
    if (typeof ResizeObserver === "undefined") return undefined;
    const ro = new ResizeObserver((entries) => {
      let changed = false;
      for (const e of entries) {
        const key = nodes.current.get(e.target);
        if (key === undefined) continue;
        // Border-box height (row padding included).
        const h = Math.ceil((e.target as HTMLElement).offsetHeight || e.contentRect.height);
        if (h > 0 && heights.current.get(key) !== h) {
          heights.current.set(key, h);
          changed = true;
        }
      }
      if (changed) setMeasureTick((t) => t + 1);
    });
    observer.current = ro;
    const el = scroller.current;
    const vo = new ResizeObserver(() => setViewport(el?.clientHeight ?? 0));
    if (el) {
      vo.observe(el);
      setViewport(el.clientHeight);
    }
    return () => {
      ro.disconnect();
      vo.disconnect();
      observer.current = null;
    };
  }, []);

  const measureRef = useCallback((key: string) => {
    return (el: HTMLDivElement | null) => {
      if (!el) return;
      nodes.current.set(el, key);
      observer.current?.observe(el);
    };
  }, []);

  const { offsets, sizes, total } = useMemo(() => {
    void measureTick;
    const offs: number[] = [];
    const hs: number[] = [];
    let acc = 0;
    for (const it of items) {
      const h = heights.current.get(getKey(it)) ?? estimate;
      offs.push(acc);
      hs.push(h);
      acc += h;
    }
    return { offsets: offs, sizes: hs, total: acc };
  }, [items, getKey, estimate, measureTick]);

  // Keep pinned to the bottom when new content arrives and the user was there.
  useLayoutEffect(() => {
    const el = scroller.current;
    if (!el || !atBottom.current) return;
    el.scrollTop = el.scrollHeight;
  }, [total, items.length, tailVersion]);

  const onScroll = (e: UIEvent<HTMLDivElement>) => {
    const el = e.currentTarget;
    atBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 48;
    setScrollTop(el.scrollTop);
  };

  // Forget nodes that left the DOM.
  useEffect(() => {
    for (const el of [...nodes.current.keys()]) {
      if (!el.isConnected) {
        observer.current?.unobserve(el);
        nodes.current.delete(el);
      }
    }
  });

  let body: ReactNode;
  if (!layoutKnown) {
    const start = Math.max(0, items.length - fallbackCount);
    body = items.slice(start).map((it) => {
      const key = getKey(it);
      return (
        <div key={key} ref={measureRef(key)} className="vlist__row">
          {render(it)}
        </div>
      );
    });
  } else {
    const [start, end] = visibleRange(offsets, sizes, scrollTop, viewport, overscan);
    const rows: ReactNode[] = [];
    for (let i = start; i <= end; i += 1) {
      const it = items[i];
      if (it === undefined) continue;
      const key = getKey(it);
      rows.push(
        <div key={key} ref={measureRef(key)} className="vlist__row" style={{ position: "absolute", top: offsets[i] ?? 0, left: 0, right: 0 }}>
          {render(it)}
        </div>,
      );
    }
    body = <div style={{ position: "relative", height: total }}>{rows}</div>;
  }

  return (
    <div ref={scroller} className={className} onScroll={onScroll} role="log" aria-live="polite" {...aria}>
      {body}
    </div>
  );
}
