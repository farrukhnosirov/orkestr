/**
 * Pure model helpers of the Architecture panel (§21, §68.48): kind order and
 * layered auto layout, search / neighbour highlighting, diff markers and the
 * standalone SVG export. No React, no I/O — unit-tested in `architecture.test.tsx`.
 *
 * Layout (left → right): services → modules (layered among themselves:
 * dependents left of their dependencies, longest path) → APIs and databases →
 * external dependency groups. Inside a column, nodes are ordered by the
 * barycenter of their neighbours in the previous column, then by name.
 */
import type { ArchDiff, ArchEdge, ArchEdgeKind, ArchNode, ArchNodeKind, ArchitectureMap } from "@orkestr/protocol";

export const NODE_KINDS: readonly ArchNodeKind[] = ["Service", "Module", "Api", "Database", "External"];
export const EDGE_KINDS: readonly ArchEdgeKind[] = ["DependsOn", "Imports", "Uses", "Exposes", "Runs", "External"];

export interface ArchLayoutOptions {
  nodeWidth: number;
  nodeHeight: number;
  columnGap: number;
  rowGap: number;
}

export const ARCH_LAYOUT: ArchLayoutOptions = { nodeWidth: 200, nodeHeight: 58, columnGap: 90, rowGap: 18 };

export interface Point {
  x: number;
  y: number;
}

/** Column of every node (see module doc). */
export function assignColumns(nodes: readonly ArchNode[], edges: readonly ArchEdge[]): Map<string, number> {
  const modules = new Set(nodes.filter((n) => n.kind === "Module").map((n) => n.id));
  // Module layering: a module sits right of every module that depends on it.
  const dependents = new Map<string, string[]>();
  for (const e of edges) {
    if ((e.kind === "DependsOn" || e.kind === "Imports") && modules.has(e.from) && modules.has(e.to) && e.from !== e.to) {
      dependents.set(e.to, [...(dependents.get(e.to) ?? []), e.from]);
    }
  }
  const depth = new Map<string, number>();
  const visiting = new Set<string>();
  const visit = (id: string): number => {
    const done = depth.get(id);
    if (done !== undefined) return done;
    if (visiting.has(id)) return -1; // cycle (imports can be cyclic): ignore the closing edge
    visiting.add(id);
    let best = 0;
    for (const d of dependents.get(id) ?? []) {
      const l = visit(d);
      if (l >= 0) best = Math.max(best, l + 1);
    }
    visiting.delete(id);
    depth.set(id, best);
    return best;
  };
  for (const id of modules) visit(id);
  const moduleDepth = modules.size > 0 ? Math.max(...depth.values()) + 1 : 0;
  const hasServices = nodes.some((n) => n.kind === "Service");
  const base = hasServices ? 1 : 0;
  const out = new Map<string, number>();
  for (const n of nodes) {
    switch (n.kind) {
      case "Service":
        out.set(n.id, 0);
        break;
      case "Module":
        out.set(n.id, base + (depth.get(n.id) ?? 0));
        break;
      case "Api":
      case "Database":
        out.set(n.id, base + moduleDepth);
        break;
      case "External":
        out.set(n.id, base + moduleDepth + 1);
        break;
    }
  }
  return out;
}

const KIND_RANK: Record<ArchNodeKind, number> = { Service: 0, Module: 1, Api: 2, Database: 3, External: 4 };

/** Positions of every node (top-left corners). */
export function layoutArchitecture(
  nodes: readonly ArchNode[],
  edges: readonly ArchEdge[],
  options: Partial<ArchLayoutOptions> = {},
): Map<string, Point> {
  const o = { ...ARCH_LAYOUT, ...options };
  const column = assignColumns(nodes, edges);
  const columns: ArchNode[][] = [];
  for (const n of nodes) {
    const c = column.get(n.id) ?? 0;
    (columns[c] ??= []).push(n);
  }
  const neighbours = new Map<string, string[]>();
  for (const e of edges) {
    neighbours.set(e.from, [...(neighbours.get(e.from) ?? []), e.to]);
    neighbours.set(e.to, [...(neighbours.get(e.to) ?? []), e.from]);
  }
  const rowOf = new Map<string, number>();
  const ordered: ArchNode[][] = [];
  for (let c = 0; c < columns.length; c++) {
    const col = columns[c] ?? [];
    const keyed = col.map((n) => {
      const rows = (neighbours.get(n.id) ?? []).map((m) => rowOf.get(m)).filter((x): x is number => x !== undefined);
      const bc = rows.length > 0 ? rows.reduce((a, b) => a + b, 0) / rows.length : Number.POSITIVE_INFINITY;
      return { n, bc };
    });
    keyed.sort(
      (a, b) =>
        KIND_RANK[a.n.kind] - KIND_RANK[b.n.kind] ||
        (a.bc === b.bc ? 0 : a.bc < b.bc ? -1 : 1) ||
        a.n.name.localeCompare(b.n.name),
    );
    const sorted = keyed.map((k) => k.n);
    sorted.forEach((n, i) => rowOf.set(n.id, i));
    ordered.push(sorted);
  }
  const tallest = Math.max(0, ...ordered.map((c) => c.length));
  const pitch = o.nodeHeight + o.rowGap;
  const positions = new Map<string, Point>();
  ordered.forEach((col, ci) => {
    const offset = ((tallest - col.length) * pitch) / 2;
    col.forEach((n, ri) => positions.set(n.id, { x: ci * (o.nodeWidth + o.columnGap), y: offset + ri * pitch }));
  });
  return positions;
}

/** Nodes and edges left after hiding kinds (edges need both ends visible). */
export function visibleGraph(
  map: Pick<ArchitectureMap, "nodes" | "edges">,
  hidden: ReadonlySet<ArchNodeKind>,
): { nodes: ArchNode[]; edges: ArchEdge[] } {
  const nodes = map.nodes.filter((n) => !hidden.has(n.kind));
  const ids = new Set(nodes.map((n) => n.id));
  return { nodes, edges: map.edges.filter((e) => ids.has(e.from) && ids.has(e.to)) };
}

/** Does `node` match the search query (name, id, path, detail, files, routes, dependencies)? */
export function matchesQuery(node: ArchNode, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  const hay = [
    node.name,
    node.id,
    node.ecosystem,
    node.path ?? "",
    node.detail ?? "",
    ...node.tags,
    ...node.files,
    ...node.dependencies,
    ...node.routes.map((r) => `${r.method} ${r.path}`),
  ];
  return hay.some((h) => h.toLowerCase().includes(q));
}

/**
 * Highlighted node ids: search matches, or the selected node plus its direct
 * neighbours. `null` = nothing to emphasise (no query, no selection).
 */
export function highlightSet(
  nodes: readonly ArchNode[],
  edges: readonly ArchEdge[],
  query: string,
  selected: string | null,
): Set<string> | null {
  if (query.trim()) return new Set(nodes.filter((n) => matchesQuery(n, query)).map((n) => n.id));
  if (!selected) return null;
  const out = new Set([selected]);
  for (const e of edges) {
    if (e.from === selected) out.add(e.to);
    if (e.to === selected) out.add(e.from);
  }
  return out;
}

export type DiffMark = "added" | "changed";

/** Node id → marker from a diff (removed nodes are not on the map). */
export function diffMarks(diff: ArchDiff | null): Map<string, DiffMark> {
  const out = new Map<string, DiffMark>();
  if (!diff) return out;
  for (const n of diff.addedNodes) out.set(n.id, "added");
  for (const c of diff.changedNodes) out.set(c.id, "changed");
  return out;
}

/** Incoming / outgoing edges of a node. */
export function edgesOf(edges: readonly ArchEdge[], id: string): { incoming: ArchEdge[]; outgoing: ArchEdge[] } {
  return { incoming: edges.filter((e) => e.to === id), outgoing: edges.filter((e) => e.from === id) };
}

/** Distinct files of a node: key files plus route files, sorted. */
export function nodeFiles(node: ArchNode): string[] {
  return [...new Set([...node.files, ...node.routes.map((r) => r.file)])].sort();
}

function escapeXml(s: string): string {
  return s.replace(/[<>&"']/g, (c) => ({ "<": "&lt;", ">": "&gt;", "&": "&amp;", '"': "&quot;", "'": "&apos;" })[c] ?? c);
}

export interface SvgTheme {
  background: string;
  text: string;
  muted: string;
  edge: string;
  kind: Record<ArchNodeKind, string>;
}

/**
 * A standalone SVG document of the map (no external assets). `kindLabel`
 * gives the localized kind captions.
 */
export function exportSvg(
  nodes: readonly ArchNode[],
  edges: readonly ArchEdge[],
  theme: SvgTheme,
  kindLabel: (k: ArchNodeKind) => string,
  options: Partial<ArchLayoutOptions> = {},
): string {
  const o = { ...ARCH_LAYOUT, ...options };
  const pos = layoutArchitecture(nodes, edges, o);
  const pad = 24;
  let w = 0;
  let h = 0;
  for (const p of pos.values()) {
    w = Math.max(w, p.x + o.nodeWidth);
    h = Math.max(h, p.y + o.nodeHeight);
  }
  const width = w + pad * 2;
  const height = h + pad * 2;
  const parts: string[] = [
    `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}" viewBox="0 0 ${width} ${height}" font-family="Segoe UI, Inter, sans-serif">`,
    `<rect width="100%" height="100%" fill="${theme.background}"/>`,
    `<defs><marker id="arrow" viewBox="0 0 10 10" refX="10" refY="5" markerWidth="7" markerHeight="7" orient="auto"><path d="M0,0 L10,5 L0,10 z" fill="${theme.edge}"/></marker></defs>`,
  ];
  for (const e of edges) {
    const a = pos.get(e.from);
    const b = pos.get(e.to);
    if (!a || !b) continue;
    const x1 = a.x + pad + o.nodeWidth;
    const y1 = a.y + pad + o.nodeHeight / 2;
    const x2 = b.x + pad;
    const y2 = b.y + pad + o.nodeHeight / 2;
    const mx = (x1 + x2) / 2;
    const dash = e.kind === "Imports" || e.kind === "External" ? ` stroke-dasharray="5 4"` : "";
    parts.push(
      `<path d="M${x1},${y1} C${mx},${y1} ${mx},${y2} ${x2},${y2}" fill="none" stroke="${theme.edge}" stroke-width="1.4"${dash} marker-end="url(#arrow)"/>`,
    );
  }
  for (const n of nodes) {
    const p = pos.get(n.id);
    if (!p) continue;
    const x = p.x + pad;
    const y = p.y + pad;
    const color = theme.kind[n.kind];
    parts.push(
      `<g><rect x="${x}" y="${y}" width="${o.nodeWidth}" height="${o.nodeHeight}" rx="8" fill="${theme.background}" stroke="${color}" stroke-width="1.5"/>`,
      `<rect x="${x}" y="${y}" width="4" height="${o.nodeHeight}" rx="2" fill="${color}"/>`,
      `<text x="${x + 12}" y="${y + 20}" font-size="10" fill="${color}">${escapeXml(kindLabel(n.kind).toUpperCase())} · ${escapeXml(n.ecosystem)}</text>`,
      `<text x="${x + 12}" y="${y + 40}" font-size="13" font-weight="600" fill="${theme.text}">${escapeXml(n.name.length > 26 ? `${n.name.slice(0, 25)}…` : n.name)}</text></g>`,
    );
  }
  parts.push("</svg>");
  return parts.join("\n");
}
