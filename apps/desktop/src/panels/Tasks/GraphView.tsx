/**
 * Visual Task Graph (§11, §68.23): layered DAG (layout in `layout.ts`) rendered
 * with @xyflow/react. Prerequisites flow left → right. Drag from a task's right
 * handle to another task's left handle to add a dependency (cycles are rejected
 * by the core and reported as a toast); select an edge and press Delete to
 * remove it. Critical-path edges glow; soft dependencies are dashed.
 */
import "@xyflow/react/dist/style.css";

import type { TaskGraph, TaskGraphNode } from "@orkestr/protocol";
import {
  Background,
  BackgroundVariant,
  type Connection,
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
} from "@xyflow/react";
import { clsx } from "clsx";
import { Route, Timer } from "lucide-react";
import { memo, useEffect, useMemo } from "react";

import { errorMessage, taskActions } from "./data";
import { DEFAULT_LAYOUT, layoutDag } from "./layout";
import { shortId, statusColor } from "./model";
import { PriorityChip } from "./parts";
import { useTasksUi } from "./store";

interface TaskNodeData extends Record<string, unknown> {
  task: TaskGraphNode;
  critical: boolean;
  ready: boolean;
}

type TaskFlowNode = Node<TaskNodeData, "task">;

const TaskNodeView = memo(function TaskNodeView({ data, selected }: NodeProps<TaskFlowNode>) {
  const t = data.task;
  return (
    <div
      className={clsx("tasks-node", selected && "tasks-node--selected", data.critical && "tasks-node--critical")}
      style={{ ["--status-color" as string]: statusColor(t.status) }}
      title={`${t.title} — ${t.status}`}
    >
      <Handle type="target" position={Position.Left} className="tasks-node__handle" />
      <div className="tasks-node__head">
        <span className="tasks-node__status">{t.status}</span>
        {data.ready && t.status !== "Ready" && <span className="tasks-node__ready">deps met</span>}
        <PriorityChip priority={t.priority} compact />
      </div>
      <div className="tasks-node__title">{t.title}</div>
      <div className="tasks-node__foot">
        <span className="tasks-mono">#{shortId(t.id)}</span>
        {t.estimateMinutes !== null && (
          <span>
            <Timer size={10} /> {t.estimateMinutes}m
          </span>
        )}
      </div>
      <Handle type="source" position={Position.Right} className="tasks-node__handle" />
    </div>
  );
});

const NODE_TYPES = { task: TaskNodeView };

function FitOnChange({ layoutKey }: { layoutKey: string }) {
  const { fitView } = useReactFlow();
  useEffect(() => {
    const id = window.requestAnimationFrame(() => void fitView({ padding: 0.2, duration: 250 }));
    return () => window.cancelAnimationFrame(id);
  }, [layoutKey, fitView]);
  return null;
}

export function GraphView({ graph }: { graph: TaskGraph }) {
  const select = useTasksUi((s) => s.select);
  const selected = useTasksUi((s) => s.selectedTaskId);
  const toast = useTasksUi((s) => s.toast);

  const layoutKey = useMemo(
    () =>
      `${graph.nodes.map((n) => n.id).join(",")}|${graph.edges.map((e) => `${e.dependsOn}>${e.taskId}`).join(",")}`,
    [graph],
  );

  const positions = useMemo(
    () =>
      layoutDag(
        graph.nodes.map((n) => n.id),
        graph.edges.map((e) => ({ from: e.dependsOn, to: e.taskId })),
      ).positions,
    // Re-layout only when the structure changes (layoutKey), not on status updates.
    [layoutKey], // eslint-disable-line react-hooks/exhaustive-deps
  );

  const criticalEdges = useMemo(() => {
    const set = new Set<string>();
    for (let i = 0; i + 1 < graph.criticalPath.length; i++) set.add(`${graph.criticalPath[i]}>${graph.criticalPath[i + 1]}`);
    return set;
  }, [graph.criticalPath]);

  const nodes = useMemo<TaskFlowNode[]>(() => {
    const critical = new Set(graph.criticalPath);
    const ready = new Set(graph.ready);
    return graph.nodes.map((n) => ({
      id: n.id,
      type: "task",
      position: positions.get(n.id) ?? { x: 0, y: 0 },
      // Controlled nodes never receive `measured` sizes back (no
      // onNodesChange), so the MiniMap needs the layout size to draw them.
      initialWidth: DEFAULT_LAYOUT.nodeWidth,
      initialHeight: DEFAULT_LAYOUT.nodeHeight,
      data: { task: n, critical: critical.has(n.id), ready: ready.has(n.id) },
      selected: n.id === selected,
      deletable: false,
      draggable: false,
    }));
  }, [graph, positions, selected]);

  const edges = useMemo<Edge[]>(
    () =>
      graph.edges.map((e) => {
        const critical = criticalEdges.has(`${e.dependsOn}>${e.taskId}`);
        return {
          id: `${e.dependsOn}>${e.taskId}`,
          source: e.dependsOn,
          target: e.taskId,
          animated: critical,
          className: clsx("tasks-edge", critical && "tasks-edge--critical", e.kind === "Soft" && "tasks-edge--soft"),
          markerEnd: { type: MarkerType.ArrowClosed, width: 16, height: 16 },
          ariaLabel: `Dependency ${shortId(e.dependsOn)} to ${shortId(e.taskId)}`,
        };
      }),
    [graph.edges, criticalEdges],
  );

  const onConnect = async (c: Connection) => {
    if (!c.source || !c.target || c.source === c.target) return;
    try {
      await taskActions.addDependency(c.target, c.source);
      toast("success", "Dependency added.");
    } catch (err) {
      toast("danger", errorMessage(err));
    }
  };

  const onEdgesDelete = async (removed: Edge[]) => {
    for (const e of removed) {
      try {
        await taskActions.removeDependency(e.target, e.source);
      } catch (err) {
        toast("danger", errorMessage(err));
      }
    }
  };

  return (
    <div className="tasks-graph" role="region" aria-label="Task dependency graph">
      <div className="tasks-graph__legend" aria-hidden="true">
        <span>
          <Route size={12} /> Critical path: {graph.criticalPathLength}m · {graph.criticalPath.length} tasks
        </span>
        <span>Drag ● → ● to add a dependency · select an edge + Delete to remove</span>
      </div>
      <ReactFlow<TaskFlowNode, Edge>
        nodes={nodes}
        edges={edges}
        nodeTypes={NODE_TYPES}
        colorMode="dark"
        fitView
        minZoom={0.2}
        maxZoom={1.75}
        nodesConnectable
        elementsSelectable
        deleteKeyCode={["Delete", "Backspace"]}
        onConnect={(c) => void onConnect(c)}
        onEdgesDelete={(es) => void onEdgesDelete(es)}
        onNodeClick={(_, n) => select(n.id)}
        onPaneClick={() => select(null)}
        isValidConnection={(c) => c.source !== c.target}
        onError={(code, message) => {
          // 004 = zero-sized container: expected while the dock hides this
          // group (e.g. another panel is maximized); it redraws when shown.
          if (code !== "004") console.warn(`[React Flow] ${message}`);
        }}
      >
        <Background variant={BackgroundVariant.Dots} gap={22} size={1} className="tasks-graph__bg" />
        <MiniMap
          pannable
          zoomable
          className="tasks-graph__minimap"
          nodeColor={(n) => statusColor((n.data as TaskNodeData).task.status)}
          nodeStrokeWidth={0}
        />
        <Controls showInteractive={false} className="tasks-graph__controls" />
        <FitOnChange layoutKey={layoutKey} />
      </ReactFlow>
    </div>
  );
}
