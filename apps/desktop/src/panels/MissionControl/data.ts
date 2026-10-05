/**
 * Data hooks of Mission Control. Everything flows through `@orkestr/client`
 * (RPC + events), so the Web/Mobile clients can reuse the same widgets.
 *
 * - `useDashboardSnapshot` — `dashboard.snapshot` (one round trip), reloaded
 *   on relevant events and every 15 s (spend has no push event).
 * - `useActiveAgents` — `agents.list {activeOnly}` + live agent events.
 * - `useMissionFeed` — alerts and build/test runs from `events.replay` +
 *   live events (one shared, reference-counted subscription).
 * - `useMiniGraph` — the DAG of the live Manager run (or the project graph).
 * - `useBudgets` — `budgets.list`.
 */
import type {
  AgentInstance,
  BudgetStatus,
  DashboardSnapshot,
  Event,
  EventKindName,
  ManagerRun,
  TaskGraph,
} from "@orkestr/protocol";
import { topic } from "@orkestr/protocol";
import { useEffect, useMemo, useState } from "react";
import { create } from "zustand";

import { maybeClient } from "../../core/client";
import { describeError } from "../../i18n/errors";
import { type RpcQuery, useRpcQuery } from "../../lib/rpcQuery";
import { useAppStore } from "../../store/app";
import { applyAgentEvent, isActive } from "../Agents/model";
import {
  type Alert,
  FEED_EVENT_KINDS,
  type QualityRun,
  alertFromEvent,
  applyQualityEvent,
  mergeAlerts,
} from "./feed";

/** `global` + every known project's topic (project events are project-scoped). */
export function useProjectTopics(): string[] {
  const projects = useAppStore((s) => s.projects);
  return useMemo(() => ["global", ...projects.map((p) => topic.project(p.id))].sort(), [projects]);
}

const SNAPSHOT_RELOAD: readonly EventKindName[] = [
  "ProjectCreated",
  "ProjectUpdated",
  "ProjectDeleted",
  "AgentSpawned",
  "AgentStopped",
  "AgentStateChanged",
  "TaskCreated",
  "TaskDeleted",
  "TaskStatusChanged",
  "ApprovalRequested",
  "ApprovalResolved",
  "ProviderStatusChanged",
  "ProviderRateLimited",
  "BudgetThreshold",
  "ManagerRunUpdated",
  "EmergencyStop",
];

/** Periodic refresh (spend, latency) while mounted. */
export const SNAPSHOT_REFRESH_MS = 15_000;

export function useDashboardSnapshot(): RpcQuery<DashboardSnapshot> {
  const topics = useProjectTopics();
  const q = useRpcQuery("dashboard.snapshot", (c) => c.call("dashboard.snapshot", {}), {
    topics,
    reloadOn: SNAPSHOT_RELOAD,
  });
  const { reload } = q;
  useEffect(() => {
    const id = window.setInterval(reload, SNAPSHOT_REFRESH_MS);
    return () => window.clearInterval(id);
  }, [reload]);
  return q;
}

export function useBudgets(): RpcQuery<BudgetStatus[]> {
  return useRpcQuery("budgets.list", async (c) => (await c.call("budgets.list", {})).budgets, {
    topics: ["global"],
    reloadOn: ["BudgetThreshold"],
  });
}

// ── active agents ──────────────────────────────────────────────────────

/** Finished agents linger this long so their completion animation is visible. */
export const LINGER_MS = 45_000;

/** Agents to show: active ones, plus the recently finished. */
export function visibleAgents(list: readonly AgentInstance[], now: number): AgentInstance[] {
  return list.filter((a) => {
    if (isActive(a.state)) return true;
    const end = Date.parse(a.stoppedAt ?? a.lastActivityAt ?? "");
    return Number.isFinite(end) && now - end < LINGER_MS;
  });
}

const AGENT_EVENTS: ReadonlySet<string> = new Set(["AgentSpawned", "AgentUpdated", "AgentStateChanged", "AgentStopped"]);

export interface ActiveAgents {
  agents: AgentInstance[];
  loading: boolean;
  error: string | null;
}

export function useActiveAgents(): ActiveAgents {
  const connected = useAppStore((s) => s.connection.state === "open");
  const topics = useProjectTopics();
  const topicsKey = topics.join("|");
  const [agents, setAgents] = useState<AgentInstance[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const client = maybeClient();
    if (!connected || !client) return undefined;
    let cancelled = false;
    setLoading(true);
    const load = () =>
      client
        .call("agents.list", { activeOnly: true })
        .then((r) => {
          if (cancelled) return;
          setAgents((prev) => {
            // Keep recently finished agents we already know about (completion animation).
            const ids = new Set(r.agents.map((a) => a.id));
            return [...r.agents, ...prev.filter((a) => !ids.has(a.id) && !isActive(a.state))];
          });
          setError(null);
        })
        .catch((e: unknown) => {
          if (!cancelled) setError(describeError(e));
        })
        .finally(() => {
          if (!cancelled) setLoading(false);
        });
    void load();
    const off = client.subscribe(topicsKey.split("|"), (ev: Event) => {
      if (!AGENT_EVENTS.has(ev.kind)) return;
      if (ev.kind === "AgentStopped") {
        void load();
        return;
      }
      setAgents((prev) => applyAgentEvent(prev, ev));
    });
    return () => {
      cancelled = true;
      off();
    };
  }, [connected, topicsKey]);

  return { agents, loading, error };
}

// ── feed (alerts + quality) ────────────────────────────────────────────

/** Persisted events scanned on start for alerts and recent runs. */
export const FEED_HISTORY = 400;

export interface FeedState {
  alerts: Alert[];
  quality: QualityRun[];
  loaded: boolean;
  /** Alert ids the user dismissed (session only). */
  dismissed: string[];
  ingest: (events: readonly Event[]) => void;
  dismiss: (id: string) => void;
  clearAll: () => void;
}

export const useFeedStore = create<FeedState>()((set) => ({
  alerts: [],
  quality: [],
  loaded: false,
  dismissed: [],
  ingest: (events) =>
    set((s) => {
      const alerts: Alert[] = [];
      let quality = s.quality;
      // Oldest first so build start/finish pair up.
      const ordered = [...events].sort((a, b) => a.seq - b.seq);
      for (const ev of ordered) {
        if (!FEED_EVENT_KINDS.has(ev.kind)) continue;
        const alert = alertFromEvent(ev);
        if (alert && !s.dismissed.includes(alert.id)) alerts.push(alert);
        quality = applyQualityEvent(quality, ev);
      }
      return { alerts: mergeAlerts(s.alerts, alerts), quality, loaded: true };
    }),
  dismiss: (id) => set((s) => ({ alerts: s.alerts.filter((a) => a.id !== id), dismissed: [...s.dismissed, id] })),
  clearAll: () => set((s) => ({ alerts: [], dismissed: [...s.dismissed, ...s.alerts.map((a) => a.id)] })),
}));

let feedConsumers = 0;
let feedStop: (() => void) | null = null;
let feedTopicsKey = "";

function startFeed(topics: readonly string[]): () => void {
  const client = maybeClient();
  if (!client) return () => {};
  let stopped = false;
  client.call("events.replay", { limit: FEED_HISTORY, newestFirst: true }).then(
    (r) => {
      if (!stopped) useFeedStore.getState().ingest(r.events);
    },
    (e: unknown) => {
      console.warn("events.replay failed", e);
      useFeedStore.setState({ loaded: true });
    },
  );
  const off = client.subscribe([...topics], (ev: Event) => {
    if (FEED_EVENT_KINDS.has(ev.kind)) useFeedStore.getState().ingest([ev]);
  });
  return () => {
    stopped = true;
    off();
  };
}

/** Joins the shared feed while mounted. */
export function useMissionFeed(): Pick<FeedState, "alerts" | "quality" | "loaded"> {
  const connected = useAppStore((s) => s.connection.state === "open");
  const topics = useProjectTopics();
  const key = topics.join("|");
  useEffect(() => {
    if (!connected) return undefined;
    feedConsumers += 1;
    if (!feedStop || feedTopicsKey !== key) {
      feedStop?.();
      feedTopicsKey = key;
      feedStop = startFeed(key.split("|"));
    }
    return () => {
      feedConsumers -= 1;
      if (feedConsumers === 0) {
        feedStop?.();
        feedStop = null;
        feedTopicsKey = "";
      }
    };
  }, [connected, key]);
  const alerts = useFeedStore((s) => s.alerts);
  const quality = useFeedStore((s) => s.quality);
  const loaded = useFeedStore((s) => s.loaded);
  return { alerts, quality, loaded };
}

// ── mini task graph ────────────────────────────────────────────────────

export interface MiniGraph {
  graph: TaskGraph | null;
  run: ManagerRun | null;
  loading: boolean;
  error: string | null;
}

/** Max nodes drawn in the widget (the Tasks panel shows the full graph). */
export const MINI_GRAPH_LIMIT = 24;

/**
 * Restricts a project graph to what the widget should draw: the live run's
 * tasks when there is one, otherwise unfinished tasks (then most recent).
 */
export function focusGraph(graph: TaskGraph, run: ManagerRun | null, limit = MINI_GRAPH_LIMIT): TaskGraph {
  let keep: Set<string>;
  if (run && run.tasks.length > 0) {
    keep = new Set(run.tasks.map((t) => t.taskId));
  } else {
    const open = graph.nodes.filter((n) => n.status !== "Completed" && n.status !== "Cancelled");
    keep = new Set((open.length > 0 ? open : graph.nodes).map((n) => n.id));
  }
  const nodes = graph.nodes.filter((n) => keep.has(n.id)).slice(-limit);
  const ids = new Set(nodes.map((n) => n.id));
  return {
    ...graph,
    nodes,
    edges: graph.edges.filter((e) => ids.has(e.taskId) && ids.has(e.dependsOn)),
    ready: graph.ready.filter((id) => ids.has(id)),
    topoOrder: graph.topoOrder.filter((id) => ids.has(id)),
    criticalPath: graph.criticalPath.filter((id) => ids.has(id)),
  };
}

const GRAPH_EVENTS: readonly EventKindName[] = ["TaskCreated", "TaskDeleted", "TaskStatusChanged", "TaskUpdated", "ManagerRunUpdated"];

export function useMiniGraph(projectId: string | null): MiniGraph {
  const q = useRpcQuery(
    `mc.graph:${projectId ?? ""}`,
    async (c) => {
      if (!projectId) return { graph: null, run: null };
      const [graph, status] = await Promise.all([
        c.call("tasks.graph", { projectId }),
        c.call("manager.status", { projectId }).catch(() => null),
      ]);
      const live = new Set(status?.liveRunIds ?? []);
      const run = status?.runs.find((r) => live.has(r.id)) ?? null;
      return { graph, run };
    },
    { enabled: projectId !== null, topics: projectId ? [topic.project(projectId)] : [], reloadOn: GRAPH_EVENTS },
  );
  const graph = q.data?.graph ? focusGraph(q.data.graph, q.data.run) : null;
  return { graph, run: q.data?.run ?? null, loading: q.loading, error: q.error };
}
