/**
 * Quality pipeline data over RPC (`pipeline.*`) plus live updates from
 * `PipelineUpdated` events. Business data comes only from the core through
 * `@orkestr/client`.
 */
import type { TaskPipeline } from "@orkestr/protocol";
import { topic } from "@orkestr/protocol";
import { useCallback, useEffect, useState } from "react";

import { getClient, maybeClient } from "../../core/client";
import { describeError } from "../../i18n/errors";
import { useAppStore } from "../../store/app";

export interface TaskPipelineView {
  pipeline: TaskPipeline | null;
  history: TaskPipeline[];
  live: boolean;
  loading: boolean;
  error: string | null;
  reload: () => void;
}

/** Latest pipeline of a task (+ earlier ones), kept live. */
export function useTaskPipeline(taskId: string): TaskPipelineView {
  const connected = useAppStore((s) => s.connection.state === "open");
  const [pipeline, setPipeline] = useState<TaskPipeline | null>(null);
  const [history, setHistory] = useState<TaskPipeline[]>([]);
  const [live, setLive] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [nonce, setNonce] = useState(0);
  const reload = useCallback(() => setNonce((n) => n + 1), []);

  useEffect(() => {
    setPipeline(null);
    setHistory([]);
    setError(null);
    const client = maybeClient();
    if (!connected || !client) return undefined;
    let cancelled = false;
    setLoading(true);
    client
      .call("pipeline.get", { taskId })
      .then((r) => {
        if (cancelled) return;
        setPipeline(r.pipeline);
        setHistory(r.history);
        setLive(r.live);
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(describeError(e));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    const off = client.subscribe([topic.task(taskId)], (ev) => {
      if (ev.kind !== "PipelineUpdated") return;
      const p = ev.payload.pipeline;
      if (p.taskId !== taskId) return;
      setPipeline((prev) => {
        if (prev && prev.id !== p.id) setHistory((h) => [prev, ...h.filter((x) => x.id !== prev.id)]);
        return p;
      });
      setLive(!isFinal(p));
    });
    return () => {
      cancelled = true;
      off();
    };
  }, [taskId, connected, nonce]);

  return { pipeline, history, live, loading, error, reload };
}

export function isFinal(p: TaskPipeline): boolean {
  return p.status === "Passed" || p.status === "Failed" || p.status === "Cancelled";
}

export const pipelineActions = {
  start: (taskId: string) => getClient().call("pipeline.start", { taskId }),
};
