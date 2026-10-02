/**
 * Tasks panel (§10–11, §68.23): list, kanban board and visual DAG of the
 * active project's tasks, a detail drawer, and task creation. All data comes
 * from the core over RPC (`tasks.*`) with live `Task*` events.
 */
import "./tasks.css";

import { Badge, Button, EmptyState, IconButton, Input, Spinner, Tabs } from "@orkestr/ui";
import { clsx } from "clsx";
import { Filter, FolderOpen, KanbanSquare, List, ListTodo, Network, Plus, RefreshCw, Search, TriangleAlert } from "lucide-react";
import { useMemo, useState } from "react";

import type { PanelProps } from "../types";
import { BoardView } from "./BoardView";
import { CreateTaskDialog } from "./CreateTaskDialog";
import { useProjectTasks } from "./data";
import { GraphView } from "./GraphView";
import { ListView } from "./ListView";
import { ALL_STATUSES, PRIORITIES, filterTasks, isFilterActive } from "./model";
import { PriorityChip, StatusBadge, Toasts } from "./parts";
import { type TasksView, useTasksUi } from "./store";
import { TaskDrawer } from "./TaskDrawer";

const VIEWS: { id: TasksView; label: string; icon: JSX.Element }[] = [
  { id: "list", label: "List", icon: <List size={14} /> },
  { id: "board", label: "Board", icon: <KanbanSquare size={14} /> },
  { id: "graph", label: "Graph", icon: <Network size={14} /> },
];

function toggle<T>(list: readonly T[], item: T): T[] {
  return list.includes(item) ? list.filter((x) => x !== item) : [...list, item];
}

export default function TasksPanel({ projectId }: PanelProps) {
  const { tasks, graph, loading, error, reload } = useProjectTasks(projectId);
  const view = useTasksUi((s) => s.view);
  const setView = useTasksUi((s) => s.setView);
  const setCreateOpen = useTasksUi((s) => s.setCreateOpen);
  const selectedId = useTasksUi((s) => s.selectedTaskId);
  const filter = useTasksUi((s) => s.filter);
  const setFilter = useTasksUi((s) => s.setFilter);
  const resetFilter = useTasksUi((s) => s.resetFilter);
  const [showFilters, setShowFilters] = useState(false);

  const visible = useMemo(() => filterTasks(tasks, filter), [tasks, filter]);
  const readyIds = useMemo(() => new Set(graph?.ready ?? []), [graph]);
  const selected = tasks.find((t) => t.id === selectedId) ?? null;
  const visibleGraph = useMemo(() => {
    if (!graph || !isFilterActive(filter)) return graph;
    const keep = new Set(visible.map((t) => t.id));
    return {
      ...graph,
      nodes: graph.nodes.filter((n) => keep.has(n.id)),
      edges: graph.edges.filter((e) => keep.has(e.taskId) && keep.has(e.dependsOn)),
    };
  }, [graph, visible, filter]);

  if (!projectId) {
    return (
      <div className="tasks-panel tasks-panel--center">
        <EmptyState
          icon={<FolderOpen size={26} strokeWidth={1.6} />}
          title="No project open"
          description="Open or create a project to plan and track its tasks."
        />
      </div>
    );
  }

  const running = tasks.filter((t) => t.status === "Running").length;
  const attention = tasks.filter((t) => t.status === "Failed" || t.status === "Interrupted").length;

  let content: JSX.Element;
  if (loading && tasks.length === 0) {
    content = (
      <div className="tasks-loading">
        <Spinner size={18} /> Loading tasks…
      </div>
    );
  } else if (error) {
    content = (
      <EmptyState
        icon={<TriangleAlert size={26} strokeWidth={1.6} />}
        title="Could not load tasks"
        description={error}
        action={
          <Button leftIcon={<RefreshCw size={14} />} onClick={reload}>
            Retry
          </Button>
        }
      />
    );
  } else if (tasks.length === 0) {
    content = (
      <EmptyState
        icon={<ListTodo size={26} strokeWidth={1.6} />}
        title="No tasks yet"
        description="Break the work into tasks with acceptance criteria and dependencies. The engine tracks every state change and unblocks dependents automatically."
        action={
          <Button variant="primary" leftIcon={<Plus size={14} />} onClick={() => setCreateOpen(true)}>
            Create task
          </Button>
        }
      />
    );
  } else if (visible.length === 0) {
    content = (
      <EmptyState
        icon={<Filter size={26} strokeWidth={1.6} />}
        title="No matching tasks"
        description="Adjust or clear the filters."
        action={<Button onClick={resetFilter}>Clear filters</Button>}
      />
    );
  } else if (view === "board") {
    content = <BoardView tasks={visible} />;
  } else if (view === "graph") {
    content = visibleGraph ? (
      <GraphView graph={visibleGraph} />
    ) : (
      <div className="tasks-loading">
        <Spinner size={18} /> Building graph…
      </div>
    );
  } else {
    content = <ListView tasks={visible} readyIds={readyIds} />;
  }

  return (
    <div className="tasks-panel">
      <div className="tasks-toolbar">
        <Button size="sm" variant="primary" leftIcon={<Plus size={14} />} onClick={() => setCreateOpen(true)}>
          New task
        </Button>
        <Tabs aria-label="Task view" items={VIEWS} value={view} onChange={setView} />
        <Input
          className="tasks-search"
          placeholder="Search tasks…"
          value={filter.query}
          onChange={(e) => setFilter({ query: e.target.value })}
          leftIcon={<Search size={14} />}
          aria-label="Search tasks"
        />
        <IconButton
          label={showFilters ? "Hide filters" : "Show filters"}
          icon={<Filter size={15} />}
          active={showFilters || isFilterActive(filter)}
          aria-expanded={showFilters}
          onClick={() => setShowFilters((s) => !s)}
          tooltip="bottom"
        />
        <div className="tasks-toolbar__spacer" />
        <div className="tasks-toolbar__stats" aria-label="Task summary">
          <Badge>{tasks.length} tasks</Badge>
          {running > 0 && <Badge tone="info">{running} running</Badge>}
          {readyIds.size > 0 && <Badge tone="accent">{readyIds.size} ready</Badge>}
          {attention > 0 && <Badge tone="danger">{attention} need attention</Badge>}
        </div>
        <IconButton label="Reload tasks" icon={<RefreshCw size={14} />} onClick={reload} tooltip="bottom" />
      </div>

      {showFilters && (
        <div className="tasks-filters" role="group" aria-label="Filters">
          <div className="tasks-filters__group">
            <span className="tasks-filters__label">Status</span>
            {ALL_STATUSES.map((s) => (
              <button
                key={s}
                type="button"
                className={clsx("tasks-chip", filter.statuses.includes(s) && "tasks-chip--on")}
                aria-pressed={filter.statuses.includes(s)}
                onClick={() => setFilter({ statuses: toggle(filter.statuses, s) })}
              >
                <StatusBadge status={s} />
              </button>
            ))}
          </div>
          <div className="tasks-filters__group">
            <span className="tasks-filters__label">Priority</span>
            {PRIORITIES.map((p) => (
              <button
                key={p}
                type="button"
                className={clsx("tasks-chip", filter.priorities.includes(p) && "tasks-chip--on")}
                aria-pressed={filter.priorities.includes(p)}
                onClick={() => setFilter({ priorities: toggle(filter.priorities, p) })}
              >
                <PriorityChip priority={p} />
              </button>
            ))}
            {isFilterActive(filter) && (
              <Button size="sm" variant="ghost" onClick={resetFilter}>
                Clear
              </Button>
            )}
          </div>
        </div>
      )}

      <div className="tasks-body">
        <div className={clsx("tasks-main", view === "graph" && "tasks-main--graph")}>{content}</div>
        {selected && <TaskDrawer task={selected} tasks={tasks} />}
      </div>
      <Toasts />
      <CreateTaskDialog projectId={projectId} tasks={tasks} />
    </div>
  );
}
