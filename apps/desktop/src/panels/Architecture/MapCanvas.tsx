/**
 * Interactive architecture map (@xyflow/react): grouped, layered nodes with
 * kind icons, edges styled by kind, search / neighbour highlighting and diff
 * markers. Layout comes from `model.ts`.
 */
import "@xyflow/react/dist/style.css";

import type { ArchEdge, ArchNode } from "@orkestr/protocol";
import {
  Background,
  BackgroundVariant,
  Controls,
  type Edge,
  Handle,
  MarkerType,
  MiniMap,
  type Node,
  type NodeProps,
  Position,
  ReactFlow,
  useReactFlow,
  useStore,
} from "@xyflow/react";
import { clsx } from "clsx";
import { memo, useEffect, useMemo } from "react";

import { useT } from "../../i18n";
import { KIND_ICON, kindClass } from "./kinds";
import { ARCH_LAYOUT, type DiffMark, layoutArchitecture } from "./model";

interface ArchNodeData extends Record<string, unknown> {
  node: ArchNode;
  dimmed: boolean;
  mark: DiffMark | null;
}

type ArchFlowNode = Node<ArchNodeData, "arch">;

const ArchNodeView = memo(function ArchNodeView({ data, selected }: NodeProps<ArchFlowNode>) {
  const { t } = useT("architecture");
  const n = data.node;
  const Icon = KIND_ICON[n.kind];
  return (
    <div
      className={clsx(
        "arch-node",
        kindClass(n.kind),
        selected && "arch-node--selected",
        data.dimmed && "arch-node--dimmed",
        data.mark && `arch-node--${data.mark}`,
      )}
      title={n.path ?? n.name}
    >
      <Handle type="target" position={Position.Left} className="arch-node__handle" isConnectable={false} />
      <div className="arch-node__head">
        <Icon size={12} />
        <span className="arch-node__kind">{t(`kind.${n.kind}`)}</span>
        <span className="arch-node__eco">{n.ecosystem}</span>
        {data.mark && <span className="arch-node__mark">{t(`diff.${data.mark}`)}</span>}
      </div>
      <div className="arch-node__name">{n.name}</div>
      <Handle type="source" position={Position.Right} className="arch-node__handle" isConnectable={false} />
    </div>
  );
});

const NODE_TYPES = { arch: ArchNodeView };

/** Fits the map when the layout changes and when the canvas is resized (maximize, dock moves). */
function FitOnChange({ layoutKey }: { layoutKey: string }) {
  const { fitView } = useReactFlow();
  // Rounded so sub-pixel jitter does not refit.
  const width = useStore((s) => Math.round(s.width / 16));
  const height = useStore((s) => Math.round(s.height / 16));
  useEffect(() => {
    if (width === 0 || height === 0) return;
    const id = window.requestAnimationFrame(() => void fitView({ padding: 0.15, duration: 250 }));
    return () => window.cancelAnimationFrame(id);
  }, [layoutKey, fitView, width, height]);
  return null;
}

export interface MapCanvasProps {
  nodes: readonly ArchNode[];
  edges: readonly ArchEdge[];
  highlight: Set<string> | null;
  marks: Map<string, DiffMark>;
  selected: string | null;
  onSelect: (id: string | null) => void;
}

export function MapCanvas({ nodes, edges, highlight, marks, selected, onSelect }: MapCanvasProps) {
  const { t } = useT("architecture");
  const layoutKey = useMemo(
    () => `${nodes.map((n) => n.id).join(",")}|${edges.map((e) => e.id).join(",")}`,
    [nodes, edges],
  );
  const positions = useMemo(
    () => layoutArchitecture(nodes, edges),
    // Re-layout only when the structure changes.
    [layoutKey], // eslint-disable-line react-hooks/exhaustive-deps
  );

  const flowNodes = useMemo<ArchFlowNode[]>(
    () =>
      nodes.map((n) => ({
        id: n.id,
        type: "arch",
        position: positions.get(n.id) ?? { x: 0, y: 0 },
        initialWidth: ARCH_LAYOUT.nodeWidth,
        initialHeight: ARCH_LAYOUT.nodeHeight,
        data: { node: n, dimmed: highlight !== null && !highlight.has(n.id), mark: marks.get(n.id) ?? null },
        selected: n.id === selected,
        draggable: false,
        deletable: false,
        connectable: false,
        ariaLabel: t("node.label", { kind: t(`kind.${n.kind}`), name: n.name }),
      })),
    [nodes, positions, highlight, marks, selected, t],
  );

  const flowEdges = useMemo<Edge[]>(() => {
    const nameOf = new Map(nodes.map((n) => [n.id, n.name]));
    return edges.map((e) => {
        const active = highlight === null || (highlight.has(e.from) && highlight.has(e.to));
        return {
          id: e.id,
          source: e.from,
          target: e.to,
          className: clsx("arch-edge", `arch-edge--${e.kind.toLowerCase()}`, !active && "arch-edge--dimmed"),
          markerEnd: { type: MarkerType.ArrowClosed, width: 14, height: 14 },
          animated: selected !== null && active && (e.from === selected || e.to === selected),
          ariaLabel: `${nameOf.get(e.from) ?? e.from} ${t(`edge.${e.kind}`)} ${nameOf.get(e.to) ?? e.to}`,
          deletable: false,
        };
    });
  }, [nodes, edges, highlight, selected, t]);

  return (
    <div className="arch-canvas" role="region" aria-label={t("map.label")}>
      <ReactFlow<ArchFlowNode, Edge>
        nodes={flowNodes}
        edges={flowEdges}
        nodeTypes={NODE_TYPES}
        colorMode="dark"
        fitView
        minZoom={0.1}
        maxZoom={1.75}
        nodesConnectable={false}
        nodesDraggable={false}
        elementsSelectable
        onNodeClick={(_, n) => onSelect(n.id)}
        onPaneClick={() => onSelect(null)}
        onError={(code, message) => {
          // 004 = zero-sized container while the dock hides this group.
          if (code !== "004") console.warn(`[React Flow] ${message}`);
        }}
      >
        <Background variant={BackgroundVariant.Dots} gap={22} size={1} className="arch-canvas__bg" />
        <MiniMap pannable zoomable className="arch-canvas__minimap" nodeStrokeWidth={0} nodeClassName={(n) => kindClass((n.data as ArchNodeData).node.kind)} />
        <Controls showInteractive={false} className="arch-canvas__controls" />
        <FitOnChange layoutKey={layoutKey} />
      </ReactFlow>
    </div>
  );
}
