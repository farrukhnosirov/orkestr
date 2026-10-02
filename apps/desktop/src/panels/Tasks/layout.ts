/**
 * Layered DAG layout for the Visual Task Graph (§11, §68.23), implemented in
 * code (no layout library):
 *
 * 1. **Layering** — longest-path from the sources: a task sits one layer to
 *    the right of its deepest prerequisite (prerequisites flow left → right).
 * 2. **Ordering** — nodes inside a layer start in input order and are then
 *    refined with a few down/up sweeps of the barycenter heuristic to reduce
 *    edge crossings.
 * 3. **Coordinates** — fixed column spacing; rows centered per layer.
 *
 * Cycles (which the core rejects, but defensive) are broken by ignoring edges
 * that close a cycle during layering.
 */

export interface LayoutEdge {
  /** Prerequisite. */
  from: string;
  /** Dependent. */
  to: string;
}

export interface LayoutOptions {
  nodeWidth: number;
  nodeHeight: number;
  columnGap: number;
  rowGap: number;
  sweeps: number;
}

export const DEFAULT_LAYOUT: LayoutOptions = { nodeWidth: 220, nodeHeight: 64, columnGap: 90, rowGap: 22, sweeps: 4 };

export interface LayoutResult {
  positions: Map<string, { x: number; y: number }>;
  layers: string[][];
}

/** Longest-path layer per node (sources = 0). */
export function assignLayers(ids: readonly string[], edges: readonly LayoutEdge[]): Map<string, number> {
  const known = new Set(ids);
  const preds = new Map<string, string[]>(ids.map((id) => [id, []]));
  for (const e of edges) {
    if (known.has(e.from) && known.has(e.to) && e.from !== e.to) preds.get(e.to)?.push(e.from);
  }
  const layer = new Map<string, number>();
  const visiting = new Set<string>();
  const visit = (id: string): number => {
    const done = layer.get(id);
    if (done !== undefined) return done;
    if (visiting.has(id)) return -1; // cycle: ignore this edge
    visiting.add(id);
    let best = 0;
    for (const p of preds.get(id) ?? []) {
      const l = visit(p);
      if (l >= 0) best = Math.max(best, l + 1);
    }
    visiting.delete(id);
    layer.set(id, best);
    return best;
  };
  for (const id of ids) visit(id);
  return layer;
}

/** Number of crossings between two adjacent layers. */
export function countCrossings(left: readonly string[], right: readonly string[], edges: readonly LayoutEdge[]): number {
  const li = new Map(left.map((id, i) => [id, i]));
  const ri = new Map(right.map((id, i) => [id, i]));
  const segs: Array<[number, number]> = [];
  for (const e of edges) {
    const a = li.get(e.from);
    const b = ri.get(e.to);
    if (a !== undefined && b !== undefined) segs.push([a, b]);
  }
  let n = 0;
  for (let i = 0; i < segs.length; i++) {
    for (let j = i + 1; j < segs.length; j++) {
      const s = segs[i];
      const t = segs[j];
      if (s && t && (s[0] - t[0]) * (s[1] - t[1]) < 0) n++;
    }
  }
  return n;
}

function totalCrossings(layers: readonly string[][], edges: readonly LayoutEdge[]): number {
  let n = 0;
  for (let i = 0; i + 1 < layers.length; i++) n += countCrossings(layers[i] ?? [], layers[i + 1] ?? [], edges);
  return n;
}

function reorder(layer: string[], neighbors: Map<string, string[]>, fixedIndex: Map<string, number>): string[] {
  const keyed = layer.map((id, i) => {
    const ns = (neighbors.get(id) ?? []).map((n) => fixedIndex.get(n)).filter((x): x is number => x !== undefined);
    const bc = ns.length > 0 ? ns.reduce((a, b) => a + b, 0) / ns.length : i;
    return { id, bc, i };
  });
  keyed.sort((a, b) => a.bc - b.bc || a.i - b.i);
  return keyed.map((k) => k.id);
}

export function layoutDag(
  ids: readonly string[],
  edges: readonly LayoutEdge[],
  options: Partial<LayoutOptions> = {},
): LayoutResult {
  const o = { ...DEFAULT_LAYOUT, ...options };
  const layerOf = assignLayers(ids, edges);
  const depth = Math.max(-1, ...layerOf.values()) + 1;
  let layers: string[][] = Array.from({ length: depth }, () => []);
  for (const id of ids) layers[layerOf.get(id) ?? 0]?.push(id);

  const preds = new Map<string, string[]>();
  const succs = new Map<string, string[]>();
  for (const e of edges) {
    if (!layerOf.has(e.from) || !layerOf.has(e.to)) continue;
    preds.set(e.to, [...(preds.get(e.to) ?? []), e.from]);
    succs.set(e.from, [...(succs.get(e.from) ?? []), e.to]);
  }

  let best = layers.map((l) => [...l]);
  let bestCross = totalCrossings(best, edges);
  for (let s = 0; s < o.sweeps && bestCross > 0; s++) {
    // Down sweep: order each layer by its predecessors' positions.
    for (let i = 1; i < layers.length; i++) {
      const fixed = new Map((layers[i - 1] ?? []).map((id, k) => [id, k]));
      layers[i] = reorder(layers[i] ?? [], preds, fixed);
    }
    // Up sweep: order by successors.
    for (let i = layers.length - 2; i >= 0; i--) {
      const fixed = new Map((layers[i + 1] ?? []).map((id, k) => [id, k]));
      layers[i] = reorder(layers[i] ?? [], succs, fixed);
    }
    const c = totalCrossings(layers, edges);
    if (c < bestCross) {
      bestCross = c;
      best = layers.map((l) => [...l]);
    }
  }
  layers = best;

  const tallest = Math.max(0, ...layers.map((l) => l.length));
  const rowPitch = o.nodeHeight + o.rowGap;
  const positions = new Map<string, { x: number; y: number }>();
  layers.forEach((layer, li) => {
    const offset = ((tallest - layer.length) * rowPitch) / 2;
    layer.forEach((id, ri) => {
      positions.set(id, { x: li * (o.nodeWidth + o.columnGap), y: offset + ri * rowPitch });
    });
  });
  return { positions, layers };
}
