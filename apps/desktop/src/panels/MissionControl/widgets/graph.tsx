import type { TaskGraph, TaskStatus } from "@orkestr/protocol";
import { clsx } from "clsx";
import { Network } from "lucide-react";
import { useMemo } from "react";

import { workspaceActions } from "../../../components/workspaceActions";
import { useT } from "../../../i18n";
import { useAppStore } from "../../../store/app";
import { layoutDag } from "../../Tasks/layout";
import { type WidgetProps, useMission } from "../context";
import { useMiniGraph } from "../data";
import { WidgetEmpty, WidgetStatus } from "./common";

const NODE_W = 132;
const NODE_H = 30;

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

function truncate(s: string, n: number): string {
  return s.length > n ? `${s.slice(0, n - 1)}…` : s;
}

function MiniDag({ graph, animate }: { graph: TaskGraph; animate: boolean }) {
  const { t } = useT(["missionControl", "common"]);
  const layout = useMemo(
    () =>
      layoutDag(
        graph.nodes.map((n) => n.id),
        graph.edges.map((e) => ({ from: e.dependsOn, to: e.taskId })),
        { nodeWidth: NODE_W, nodeHeight: NODE_H, columnGap: 34, rowGap: 10, sweeps: 3 },
      ),
    [graph],
  );
  const pos = layout.positions;
  let w = 0;
  let h = 0;
  for (const p of pos.values()) {
    w = Math.max(w, p.x + NODE_W);
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
        const x1 = a.x + NODE_W;
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
            <rect width={NODE_W} height={NODE_H} rx={7} className="mc-dag__box" />
            <circle cx={12} cy={NODE_H / 2} r={4} className="mc-dag__dot" />
            <text x={22} y={NODE_H / 2 + 4} className="mc-dag__text">
              {truncate(n.title, 17)}
            </text>
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
  if (!focusProjectId) return <WidgetEmpty icon={<Network size={20} />} title={t("graph.noProject")} />;
  if (!graph) return <WidgetStatus loading={loading || !error} error={error} />;
  return (
    <div className="mc-graph">
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
        <MiniDag graph={graph} animate={budget.animate} />
      )}
    </div>
  );
}
