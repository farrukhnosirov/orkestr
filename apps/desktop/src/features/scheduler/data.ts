/**
 * Scheduler data over RPC (`scheduler.status`, `scheduler.setLimits`,
 * `scheduler.explain`) with live refresh on `SchedulerUpdated` (topic
 * `global`) and task / merge-risk events of the project. A slow poll keeps
 * time-based facts (rate-limit back-off, host resources) fresh.
 */
import type { SchedulerExplanation, SchedulerSettings, SchedulerStatus } from "@orkestr/protocol";
import { topic } from "@orkestr/protocol";
import { useCallback, useEffect, useRef, useState } from "react";

import { getClient, maybeClient } from "../../core/client";
import { describeError } from "../../i18n/errors";
import { useAppStore } from "../../store/app";

/** Events after which the scheduler picture may have changed. */
export const REFRESH_EVENTS = new Set([
  "SchedulerUpdated",
  "MergeRiskDetected",
  "TaskStatusChanged",
  "ProviderRateLimited",
  "ManagerRunUpdated",
]);

/** Poll interval for time-based facts. */
export const POLL_MS = 10_000;

export interface SchedulerData {
  status: SchedulerStatus | null;
  loading: boolean;
  error: string | null;
  reload: () => void;
}

/** Live scheduler status (filtered to `projectId` when given; whole core otherwise). */
export function useSchedulerStatus(projectId: string | null, enabled = true): SchedulerData {
  const connected = useAppStore((s) => s.connection.state === "open");
  const [status, setStatus] = useState<SchedulerStatus | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [nonce, setNonce] = useState(0);
  const timer = useRef<number | null>(null);
  const reload = useCallback(() => setNonce((n) => n + 1), []);

  useEffect(() => {
    if (!enabled || !connected) return undefined;
    const client = maybeClient();
    if (!client) return undefined;
    let cancelled = false;
    const params = projectId ? { projectId } : {};
    const load = () => {
      client
        .call("scheduler.status", params)
        .then((s) => {
          if (cancelled) return;
          setStatus(s);
          setError(null);
        })
        .catch((e: unknown) => {
          if (!cancelled) setError(describeError(e));
        })
        .finally(() => {
          if (!cancelled) setLoading(false);
        });
    };
    const schedule = () => {
      if (timer.current !== null) window.clearTimeout(timer.current);
      timer.current = window.setTimeout(load, 200);
    };
    setLoading(true);
    load();
    const topics = projectId ? [topic.global, topic.project(projectId)] : [topic.global];
    const off = client.subscribe(topics, (ev) => {
      if (REFRESH_EVENTS.has(ev.kind)) schedule();
    });
    const poll = window.setInterval(load, POLL_MS);
    return () => {
      cancelled = true;
      off();
      window.clearInterval(poll);
      if (timer.current !== null) window.clearTimeout(timer.current);
    };
  }, [projectId, connected, enabled, nonce]);

  return { status, loading, error, reload };
}

export const schedulerActions = {
  setLimits: (settings: SchedulerSettings) =>
    getClient()
      .call("scheduler.setLimits", { settings })
      .then((r) => r.settings),
  explain: (taskId: string): Promise<SchedulerExplanation> => getClient().call("scheduler.explain", { taskId }),
};
