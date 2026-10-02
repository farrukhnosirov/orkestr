/**
 * Task data for a project: initial load over RPC (`tasks.list`, `tasks.graph`)
 * plus live updates from `project:<id>` events (`TaskCreated`, `TaskUpdated`,
 * `TaskStatusChanged`, `TaskDeleted`). Mutations go through `taskActions`.
 */
import { OrkestrError } from "@orkestr/client";
import type {
  CreateTaskParams,
  DependencyKind,
  Event,
  Task,
  TaskChangeResult,
  TaskGraph,
  TaskPatch,
  TaskStatus,
} from "@orkestr/protocol";
import { topic } from "@orkestr/protocol";
import { useCallback, useEffect, useRef, useState } from "react";

import { getClient, maybeClient } from "../../core/client";
import { useAppStore } from "../../store/app";
import { upsertTask } from "./model";

export interface ProjectTasks {
  tasks: Task[];
  graph: TaskGraph | null;
  loading: boolean;
  error: string | null;
  reload: () => void;
}

/** Human message for a failed call. */
export function errorMessage(e: unknown): string {
  if (e instanceof OrkestrError) {
    if (e.code === "conflict" && e.data && typeof e.data === "object" && "cycle" in e.data) {
      return "That dependency would create a cycle.";
    }
    return e.message;
  }
  return e instanceof Error ? e.message : String(e);
}

/** Applies one event to a task list. Returns the same array when nothing changed. */
export function applyTaskEvent(tasks: Task[], ev: Event): Task[] {
  switch (ev.kind) {
    case "TaskCreated":
    case "TaskUpdated":
      return upsertTask(tasks, ev.payload.task);
    case "TaskDeleted":
      return tasks.filter((t) => t.id !== ev.payload.taskId);
    default:
      return tasks;
  }
}

const TASK_EVENTS = new Set(["TaskCreated", "TaskUpdated", "TaskStatusChanged", "TaskDeleted"]);

export function useProjectTasks(projectId: string | null): ProjectTasks {
  const connected = useAppStore((s) => s.connection.state === "open");
  const [tasks, setTasks] = useState<Task[]>([]);
  const [graph, setGraph] = useState<TaskGraph | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [nonce, setNonce] = useState(0);
  const graphTimer = useRef<number | null>(null);

  const reload = useCallback(() => setNonce((n) => n + 1), []);

  useEffect(() => {
    setTasks([]);
    setGraph(null);
    setError(null);
    if (!projectId || !connected) return;
    const client = maybeClient();
    if (!client) return;
    let cancelled = false;

    const loadGraph = () => {
      client
        .call("tasks.graph", { projectId })
        .then((g) => {
          if (!cancelled) setGraph(g);
        })
        .catch((e: unknown) => {
          if (!cancelled) console.warn("tasks.graph failed", e);
        });
    };
    const scheduleGraph = () => {
      if (graphTimer.current !== null) window.clearTimeout(graphTimer.current);
      graphTimer.current = window.setTimeout(loadGraph, 120);
    };

    setLoading(true);
    Promise.all([client.call("tasks.list", { projectId }), client.call("tasks.graph", { projectId })])
      .then(([list, g]) => {
        if (cancelled) return;
        setTasks(list.tasks);
        setGraph(g);
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(errorMessage(e));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });

    const off = client.subscribe([topic.project(projectId)], (ev) => {
      if (!TASK_EVENTS.has(ev.kind)) return;
      setTasks((prev) => applyTaskEvent(prev, ev));
      scheduleGraph();
    });
    return () => {
      cancelled = true;
      off();
      if (graphTimer.current !== null) window.clearTimeout(graphTimer.current);
    };
  }, [projectId, connected, nonce]);

  return { tasks, graph, loading, error, reload };
}

/** RPC mutations (results also arrive as events; callers may use the return value). */
export const taskActions = {
  create: (params: CreateTaskParams) => getClient().call("tasks.create", params).then((r) => r.task),
  update: (taskId: string, patch: TaskPatch, expectedVersion?: number) =>
    getClient()
      .call("tasks.update", expectedVersion === undefined ? { taskId, patch } : { taskId, patch, expectedVersion })
      .then((r) => r.task),
  remove: (taskId: string) => getClient().call("tasks.delete", { taskId }),
  transition: (taskId: string, to: TaskStatus, reason?: string): Promise<TaskChangeResult> =>
    getClient().call("tasks.transition", reason ? { taskId, to, reason } : { taskId, to }),
  retry: (taskId: string) => getClient().call("tasks.retry", { taskId }),
  addDependency: (taskId: string, dependsOn: string, kind?: DependencyKind) =>
    getClient().call("tasks.addDependency", kind ? { taskId, dependsOn, kind } : { taskId, dependsOn }),
  removeDependency: (taskId: string, dependsOn: string) =>
    getClient().call("tasks.removeDependency", { taskId, dependsOn }),
  history: (taskId: string) => getClient().call("tasks.history", { taskId }),
};
