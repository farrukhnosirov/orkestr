/**
 * Keeps each dock group's active tab visible in its tab strip.
 *
 * dockview scrolls a tab into view only when it becomes active. When a group
 * shrinks afterwards (restore after maximize, splitter drag, window resize,
 * preset switch) the strip keeps its old `scrollLeft`, so the active tab can
 * sit past the visible edge while an inactive tab reads as the group title.
 * After such layout changes every group's strip is scrolled to its active tab.
 */

export interface StripMetrics {
  /** Tab start / width inside the strip (offsetLeft / offsetWidth). */
  tabStart: number;
  tabSize: number;
  /** Strip scroll position and visible width (scrollLeft / clientWidth). */
  scroll: number;
  visible: number;
}

/** New scroll position that shows the whole tab, or `null` when it is already visible. */
export function revealScroll(m: StripMetrics): number | null {
  const end = m.tabStart + m.tabSize;
  if (m.tabStart >= m.scroll && end <= m.scroll + m.visible) return null;
  // Wider than the strip, or left of it: align the start; right of it: align the end.
  if (m.tabSize >= m.visible || m.tabStart < m.scroll) return Math.max(0, m.tabStart);
  return Math.max(0, end - m.visible);
}

/** Scrolls every tab strip under `root` to its active tab. */
export function revealActiveTabs(root: ParentNode): void {
  for (const tab of Array.from(root.querySelectorAll<HTMLElement>(".dv-tab.dv-active-tab"))) {
    const strip = tab.parentElement;
    if (!strip) continue;
    const next = revealScroll({ tabStart: tab.offsetLeft, tabSize: tab.offsetWidth, scroll: strip.scrollLeft, visible: strip.clientWidth });
    if (next !== null) strip.scrollLeft = next;
  }
}

/** Coalesces reveal requests to one pass per frame (layout must have settled). */
export function createTabRevealer(root: () => ParentNode | null): { request: () => void; dispose: () => void } {
  let frame = 0;
  const raf = typeof window !== "undefined" && typeof window.requestAnimationFrame === "function";
  const run = () => {
    frame = 0;
    const r = root();
    if (r) revealActiveTabs(r);
  };
  return {
    request: () => {
      if (!raf) return run();
      if (frame) return;
      frame = window.requestAnimationFrame(run);
    },
    dispose: () => {
      if (frame && raf) window.cancelAnimationFrame(frame);
      frame = 0;
    },
  };
}
