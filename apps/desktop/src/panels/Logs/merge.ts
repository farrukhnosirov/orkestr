import type { Event } from "@orkestr/protocol";

function tsOf(ev: Event): number {
  const n = Date.parse(ev.ts);
  return Number.isNaN(n) ? 0 : n;
}

/**
 * Merges `incoming` into the already ordered `prev` list: dedupes by id and
 * keeps the list in ascending core `seq` order, whatever order events arrived
 * in (live events can arrive before the history page that precedes them).
 * Local events without a core sequence (`seq === 0`) keep their place by
 * timestamp: each goes after the last sequenced event that is not newer.
 * Only the newest `max` events are kept.
 */
export function mergeEvents(prev: Event[], incoming: Event[], max: number): Event[] {
  if (incoming.length === 0) return prev;
  const seen = new Set<string>();
  const all: Event[] = [];
  for (const ev of [...prev, ...incoming]) {
    if (seen.has(ev.id)) continue;
    seen.add(ev.id);
    all.push(ev);
  }
  const sequenced = all.filter((e) => e.seq > 0).sort((a, b) => a.seq - b.seq);
  const local = all.filter((e) => e.seq <= 0);
  let merged: Event[];
  if (local.length === 0) {
    merged = sequenced;
  } else {
    // Stable by timestamp, then by original order (Array.prototype.sort is stable).
    local.sort((a, b) => tsOf(a) - tsOf(b));
    merged = [];
    let li = 0;
    for (const ev of sequenced) {
      const t = tsOf(ev);
      while (li < local.length && tsOf(local[li]!) < t) merged.push(local[li++]!);
      merged.push(ev);
    }
    while (li < local.length) merged.push(local[li++]!);
  }
  return merged.length > max ? merged.slice(merged.length - max) : merged;
}
