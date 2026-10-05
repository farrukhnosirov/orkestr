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

import { SchedulerBar } from "../../features/scheduler/SchedulerBar";
import { useT } from "../../i18n";
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

const VIEW_ICONS: Record<TasksView, JSX.Element> = {
  list: <List size={14} />,
  board: <KanbanSquare size={14} />,
  graph: <Network size={14} />,
};
const VIEW_IDS: readonly TasksView[] = ["list", "board", "graph"];

function toggle<T>(list: readonly T[], item: T): T[] {
  return list.includes(item) ? list.filter((x) => x !== item) : [...list, item];
}

export default function TasksPanel({ projectId }: PanelProps) {
  const { t } = useT(["tasks", "common"]);
  const { tasks, graph, loading, error, reload } = useProjectTasks(projectId);
  const view = useTasksUi((s) => s.view);
  const setView = useTasksUi((s) => s.setView);
  const setCreateOpen = useTasksUi((s) => s.setCreateOpen);
  const selectedId = useTasksUi((s) => s.selectedTaskId);
  const filter = useTasksUi((s) => s.filter);
  const setFilter = useTasksUi((s) => s.setFilter);
  const resetFilter = useTasksUi((s) => s.resetFilter);
  const [showFilters, setShowFilters] = useState(false);
  const views = VIEW_IDS.map((id) => ({ id, label: t(`views.${id}`), icon: VIEW_ICONS[id] }));

  const visible = useMemo(() => filterTasks(tasks, filter), [tasks, filter]);
  const readyIds = useMemo(() => new Set(graph?.ready ?? []), [graph]);
  const selected = tasks.find((t) => t.id === selectedId) ?? null;
  const titles = useMemo(() => new Map(tasks.map((x) => [x.id, x.title] as const)), [tasks]);
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
          title={t("noProject.title")}
          description={t("noProject.description")}
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
        <Spinner size={18} /> {t("loading")}
      </div>
    );
  } else if (error) {
    content = (
      <EmptyState
        icon={<TriangleAlert size={26} strokeWidth={1.6} />}
        title={t("loadFailed")}
        description={error}
        action={
          <Button leftIcon={<RefreshCw size={14} />} onClick={reload}>
            {t("common:actions.retry")}
          </Button>
        }
      />
    );
  } else if (tasks.length === 0) {
    content = (
      <EmptyState
        icon={<ListTodo size={26} strokeWidth={1.6} />}
        title={t("empty.title")}
        description={t("empty.description")}
        action={
          <Button variant="primary" leftIcon={<Plus size={14} />} onClick={() => setCreateOpen(true)}>
            {t("create")}
          </Button>
        }
      />
    );
  } else if (visible.length === 0) {
    content = (
      <EmptyState
        icon={<Filter size={26} strokeWidth={1.6} />}
        title={t("noMatches.title")}
        description={t("noMatches.description")}
        action={<Button onClick={resetFilter}>{t("clearFilters")}</Button>}
      />
    );
  } else if (view === "board") {
    content = <BoardView tasks={visible} />;
  } else if (view === "graph") {
    content = visibleGraph ? (
      <GraphView graph={visibleGraph} />
    ) : (
      <div className="tasks-loading">
        <Spinner size={18} /> {t("buildingGraph")}
      </div>
    );
  } else {
    content = <ListView tasks={visible} readyIds={readyIds} />;
  }

  return (
    <div className="tasks-panel">
      <div className="tasks-toolbar">
        <Button size="sm" variant="primary" leftIcon={<Plus size={14} />} onClick={() => setCreateOpen(true)}>
          {t("new")}
        </Button>
        <Tabs aria-label={t("views.label")} items={views} value={view} onChange={setView} />
        <Input
          className="tasks-search"
          placeholder={t("search.placeholder")}
          value={filter.query}
          onChange={(e) => setFilter({ query: e.target.value })}
          leftIcon={<Search size={14} />}
          aria-label={t("search.label")}
        />
        <IconButton
          label={showFilters ? t("filters.hide") : t("filters.show")}
          icon={<Filter size={15} />}
          active={showFilters || isFilterActive(filter)}
          aria-expanded={showFilters}
          onClick={() => setShowFilters((s) => !s)}
          tooltip="bottom"
        />
        <div className="tasks-toolbar__spacer" />
        <div className="tasks-toolbar__stats" aria-label={t("summary.label")}>
          <Badge>{t("common:count.tasks", { count: tasks.length })}</Badge>
          {running > 0 && <Badge tone="info">{t("summary.running", { count: running })}</Badge>}
          {readyIds.size > 0 && <Badge tone="accent">{t("summary.ready", { count: readyIds.size })}</Badge>}
          {attention > 0 && <Badge tone="danger">{t("summary.attention", { count: attention })}</Badge>}
        </div>
        <IconButton label={t("reload")} icon={<RefreshCw size={14} />} onClick={reload} tooltip="bottom" />
      </div>

      <SchedulerBar projectId={projectId} titles={titles} />

      {showFilters && (
        <div className="tasks-filters" role="group" aria-label={t("filters.label")}>
          <div className="tasks-filters__group">
            <span className="tasks-filters__label">{t("filters.status")}</span>
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
            <span className="tasks-filters__label">{t("filters.priority")}</span>
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
                {t("clear")}
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
