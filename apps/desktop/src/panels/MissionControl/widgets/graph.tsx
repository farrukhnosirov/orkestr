import type { TaskGraph, TaskStatus } from "@orkestr/protocol";
import { clsx } from "clsx";
import { Network } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import { workspaceActions } from "../../../components/workspaceActions";
import { useT } from "../../../i18n";
import { useAppStore } from "../../../store/app";
import { layoutDag } from "../../Tasks/layout";
import { type WidgetProps, useMission } from "../context";
import { useMiniGraph } from "../data";
import { WidgetEmpty, WidgetStatus } from "./common";

/** Narrowest node; the graph scales down (viewBox) when even this does not fit. */
const NODE_W = 132;
/** Widest node: wider boxes stop helping legibility. */
const NODE_W_MAX = 280;
const NODE_H = 30;
const COLUMN_GAP = 34;
/** Space left of the title (status dot) and right padding inside a node. */
const TEXT_X = 22;
const TEXT_PAD_RIGHT = 8;

/**
 * Node width for a DAG of `depth` columns in `available` px: the columns
 * share the widget width (so a wide widget shows longer titles), clamped to
 * [132, 280]. Unknown width → the minimum.
 */
export function graphNodeWidth(available: number | null | undefined, depth: number): number {
  if (!available || !Number.isFinite(available) || available <= 0 || depth <= 0) return NODE_W;
  const per = (available - 8 - (depth - 1) * COLUMN_GAP) / depth;
  return Math.max(NODE_W, Math.min(NODE_W_MAX, Math.floor(per)));
}

/** Measured width of an element attached through the returned callback ref (it may mount later). */
function useWidth(): [(el: HTMLDivElement | null) => void, number | null] {
  const [el, setEl] = useState<HTMLDivElement | null>(null);
  const [width, setWidth] = useState<number | null>(null);
  useEffect(() => {
    if (!el) return undefined;
    setWidth(el.clientWidth || null);
    if (typeof ResizeObserver === "undefined") return undefined;
    const ro = new ResizeObserver((entries) => {
      const w = entries[0]?.contentRect.width;
      if (w) setWidth(w);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, [el]);
  return [setEl, width];
}

/** Visual group of a task status (CSS class suffix). */
export function statusGroup(s: TaskStatus): "done" | "running" | "waiting" | "failed" | "idle" | "cancelled" {
  switch (s) {
    case "Completed":
      return "done";
    case "Running":
    case "Review":
    case "Testing":
    case "Recovering":
      return "running";
    case "Waiting":
    case "Blocked":
      return "waiting";
    case "Failed":
    case "Interrupted":
      return "failed";
    case "Cancelled":
      return "cancelled";
    default:
      return "idle";
  }
}

function MiniDag({ graph, animate, available }: { graph: TaskGraph; animate: boolean; available: number | null }) {
  const { t } = useT(["missionControl", "common"]);
  const ids = useMemo(() => graph.nodes.map((n) => n.id), [graph]);
  const edges = useMemo(() => graph.edges.map((e) => ({ from: e.dependsOn, to: e.taskId })), [graph]);
  // The column count does not depend on the node width.
  const depth = useMemo(
    () => layoutDag(ids, edges, { nodeWidth: NODE_W, nodeHeight: NODE_H, columnGap: COLUMN_GAP, rowGap: 10, sweeps: 0 }).layers.length,
    [ids, edges],
  );
  const nodeW = graphNodeWidth(available, depth);
  const layout = useMemo(
    () => layoutDag(ids, edges, { nodeWidth: nodeW, nodeHeight: NODE_H, columnGap: COLUMN_GAP, rowGap: 10, sweeps: 3 }),
    [ids, edges, nodeW],
  );
  const pos = layout.positions;
  let w = 0;
  let h = 0;
  for (const p of pos.values()) {
    w = Math.max(w, p.x + nodeW);
    h = Math.max(h, p.y + NODE_H);
  }
  const critical = new Set(graph.criticalPath);
  return (
    <svg
      className={clsx("mc-dag", animate && "mc-dag--animate")}
      viewBox={`-4 -4 ${w + 8} ${h + 8}`}
      preserveAspectRatio="xMidYMid meet"
      style={{ maxWidth: w + 8, maxHeight: h + 8 }}
      role="img"
      aria-label={t("graph.label", { count: graph.nodes.length })}
    >
      {graph.edges.map((e) => {
        const a = pos.get(e.dependsOn);
        const b = pos.get(e.taskId);
        if (!a || !b) return null;
        const x1 = a.x + nodeW;
        const y1 = a.y + NODE_H / 2;
        const x2 = b.x;
        const y2 = b.y + NODE_H / 2;
        const mx = (x1 + x2) / 2;
        return (
          <path
            key={`${e.dependsOn}-${e.taskId}`}
            d={`M${x1},${y1} C${mx},${y1} ${mx},${y2} ${x2},${y2}`}
            className={clsx("mc-dag__edge", critical.has(e.taskId) && critical.has(e.dependsOn) && "mc-dag__edge--critical")}
          />
        );
      })}
      {graph.nodes.map((n) => {
        const p = pos.get(n.id);
        if (!p) return null;
        const group = statusGroup(n.status);
        return (
          <g key={n.id} transform={`translate(${p.x},${p.y})`} className={clsx("mc-dag__node", `mc-dag__node--${group}`)}>
            <title>{`${n.title} — ${t(`common:status.task.${n.status}`)}`}</title>
            <rect width={nodeW} height={NODE_H} rx={7} className="mc-dag__box" />
            <circle cx={12} cy={NODE_H / 2} r={4} className="mc-dag__dot" />
            {/* Pixel-accurate ellipsis (CSS text-overflow) instead of a fixed character count. */}
            <foreignObject x={TEXT_X} y={0} width={Math.max(0, nodeW - TEXT_X - TEXT_PAD_RIGHT)} height={NODE_H}>
              <div className="mc-dag__text" style={{ lineHeight: `${NODE_H}px` }}>
                {n.title}
              </div>
            </foreignObject>
          </g>
        );
      })}
    </svg>
  );
}

/** Live DAG of the active run (or the project's open tasks). */
export function TaskGraphWidget(_props: WidgetProps) {
  const { t } = useT("missionControl");
  const { focusProjectId, budget } = useMission();
  const projects = useAppStore((s) => s.projects);
  const { graph, run, loading, error } = useMiniGraph(focusProjectId);
  const project = projects.find((p) => p.id === focusProjectId);
  const [boxRef, available] = useWidth();
  if (!focusProjectId) return <WidgetEmpty icon={<Network size={20} />} title={t("graph.noProject")} />;
  if (!graph) return <WidgetStatus loading={loading || !error} error={error} />;
  return (
    <div className="mc-graph" ref={boxRef}>
      <div className="mc-graph__head">
        <span className="mc-graph__project">{project?.name ?? ""}</span>
        <span className="mc-graph__run">{run
            ? t("graph.liveRun", { goal: run.goal })
            : graph.nodes.length > 0 && graph.nodes.every((n) => n.status === "Completed" || n.status === "Cancelled")
              ? t("graph.recentTasks")
              : t("graph.openTasks")}</span>
        <button type="button" className="mc-link" onClick={() => workspaceActions.openPanel("tasks")}>
          {t("graph.openTasksPanel")}
        </button>
      </div>
      {graph.nodes.length === 0 ? (
        <WidgetEmpty icon={<Network size={20} />} title={t("graph.empty")} />
      ) : (
        <MiniDag graph={graph} animate={budget.animate} available={available} />
      )}
    </div>
  );
}
