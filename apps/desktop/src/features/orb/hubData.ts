/**
 * Agent Hub data (RPC + events only, ADR-0045): every Orkestr agent and
 * every live Claude Code session across all projects, kept fresh by agent
 * events (debounced reload) and a slow poll; plus the hub's actions.
 * Approvals and the emergency flags come from the security store
 * (`initSecurityLive`, started by the orb window).
 */
import type { Event } from "@orkestr/protocol";
import { topic } from "@orkestr/protocol";
import { useEffect, useMemo, useState } from "react";

import { getClient, maybeClient } from "../../core/client";
import { agentName } from "../../lib/coreText";
import { useAppStore } from "../../store/app";
import { type HubItem, agentItem, claudeItem, managerRelayText, visibleItems } from "./hubModel";
import { orbShell } from "./shell";

const AGENT_EVENTS = new Set(["AgentSpawned", "AgentUpdated", "AgentStateChanged", "AgentStopped"]);

export interface HubData {
  items: HubItem[];
  loaded: boolean;
  error: string | null;
  reload: () => void;
}

/** All agents + Claude Code sessions (visible ones: live or recently finished). */
export function useHubData(fast: boolean): HubData {
  const connected = useAppStore((s) => s.connection.state === "open");
  const projectKey = useAppStore((s) => s.projects.map((p) => p.id).join(","));
  const [agents, setAgents] = useState<HubItem[]>([]);
  const [sessions, setSessions] = useState<HubItem[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [nonce, setNonce] = useState(0);
  const [now, setNow] = useState(() => Date.now());

  // Orkestr agents: one list call, reloaded (debounced) on agent events.
  useEffect(() => {
    if (!connected) return undefined;
    const c = maybeClient();
    if (!c) return undefined;
    let cancelled = false;
    let timer = 0;
    const load = () => {
      c.call("agents.list", {}).then(
        (r) => {
          if (cancelled) return;
          setAgents(r.agents.map(agentItem));
          setLoaded(true);
          setError(null);
          setNow(Date.now());
        },
        (e: unknown) => {
          if (!cancelled) setError(e instanceof Error ? e.message : String(e));
        },
      );
    };
    load();
    const ids = projectKey ? projectKey.split(",") : [];
    const off = ids.length
      ? c.subscribe(
          ids.map((id) => topic.project(id)),
          (ev: Event) => {
            if (!AGENT_EVENTS.has(ev.kind)) return;
            window.clearTimeout(timer);
            timer = window.setTimeout(load, 250);
          },
        )
      : () => {};
    const poll = window.setInterval(load, 20_000);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
      window.clearInterval(poll);
      off();
    };
  }, [connected, projectKey, nonce]);

  // Claude Code sessions per project (polled: their events live on per-session topics).
  useEffect(() => {
    if (!connected || !projectKey) {
      setSessions([]);
      return undefined;
    }
    const c = maybeClient();
    if (!c) return undefined;
    let cancelled = false;
    const ids = projectKey.split(",");
    const load = () => {
      Promise.allSettled(ids.map((projectId) => c.call("claudeCode.sessions", { projectId }))).then((results) => {
        if (cancelled) return;
        const live = results.flatMap((r) => (r.status === "fulfilled" ? r.value.live.map(claudeItem) : []));
        setSessions(live);
      });
    };
    load();
    const poll = window.setInterval(load, fast ? 4000 : 20_000);
    return () => {
      cancelled = true;
      window.clearInterval(poll);
    };
  }, [connected, projectKey, fast, nonce]);

  // The "recently finished" window moves with time.
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 60_000);
    return () => window.clearInterval(timer);
  }, []);

  const items = useMemo(() => visibleItems([...agents, ...sessions], now), [agents, sessions, now]);
  return { items, loaded, error, reload: () => setNonce((n) => n + 1) };
}

/** Actions on hub items. Each resolves when the core accepted the request. */
export const hubActions = {
  async message(item: HubItem, text: string): Promise<void> {
    const value = text.trim();
    if (!value) return;
    if (item.kind === "claude") {
      await getClient().call("claudeCode.send", { sessionId: item.id, text: value });
      return;
    }
    // Orkestr agents take instructions through their project's Manager.
    await getClient().call("manager.chat", { projectId: item.projectId, text: managerRelayText(agentName(item.name), item.id, value) });
  },
  async messageManager(projectId: string, text: string): Promise<void> {
    const value = text.trim();
    if (value) await getClient().call("manager.chat", { projectId, text: value });
  },
  async stop(item: HubItem): Promise<void> {
    if (item.kind === "claude") {
      if (item.status === "working" || item.status === "waiting") await getClient().call("claudeCode.interrupt", { sessionId: item.id });
      else await getClient().call("claudeCode.stop", { sessionId: item.id });
      return;
    }
    await getClient().call("agents.stop", { agentId: item.id });
  },
  async pause(item: HubItem): Promise<void> {
    if (item.kind === "agent") await getClient().call("agents.pause", { agentId: item.id });
  },
  async resume(item: HubItem): Promise<void> {
    if (item.kind === "agent") await getClient().call("agents.resume", { agentId: item.id });
  },
  /** Spawns a fresh agent from the same template (on the same task when it had one). */
  async restart(item: HubItem): Promise<void> {
    if (item.kind !== "agent" || !item.templateId) return;
    await getClient().call("agents.spawn", item.taskId ? { projectId: item.projectId, templateId: item.templateId, taskId: item.taskId } : { projectId: item.projectId, templateId: item.templateId });
  },
  /** Opens the agent / chat in the main window. */
  async transcript(item: HubItem): Promise<void> {
    if (item.kind === "claude") await orbShell.toMain("open-claude", { sessionId: item.id, projectId: item.projectId, claudeSessionId: item.claudeSessionId });
    else await orbShell.toMain("open-agent", { agentId: item.id, projectId: item.projectId });
  },
};
