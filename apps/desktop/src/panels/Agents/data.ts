/**
 * Agents data: initial loads over RPC (`agents.*`, `templates.*`) plus live
 * events (`project:<id>` for the list, `agent:<id>` for the transcript).
 * No business data is stored outside these hooks.
 */
import type {
  AgentInstance,
  AgentStopMode,
  AgentTemplate,
  Event,
  SpawnAgentParams,
  TemplateDraft,
} from "@orkestr/protocol";
import { topic } from "@orkestr/protocol";
import { useCallback, useEffect, useRef, useState } from "react";

import { getClient, maybeClient } from "../../core/client";
import { describeError } from "../../i18n/errors";
import { useAppStore } from "../../store/app";
import { EMPTY_TRANSCRIPT, applyAgentEvent, applyTranscriptEvent, mergeTranscript, type TranscriptState } from "./model";

/** Localized message for a failed call (see `i18n/errors.ts`). */
export function errorMessage(e: unknown): string {
  return describeError(e);
}

const AGENT_EVENTS = new Set(["AgentSpawned", "AgentUpdated", "AgentStateChanged", "AgentStopped"]);

export interface ProjectAgents {
  agents: AgentInstance[];
  loading: boolean;
  error: string | null;
  reload: () => void;
}

export function useProjectAgents(projectId: string | null): ProjectAgents {
  const connected = useAppStore((s) => s.connection.state === "open");
  const [agents, setAgents] = useState<AgentInstance[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [nonce, setNonce] = useState(0);
  const reload = useCallback(() => setNonce((n) => n + 1), []);

  useEffect(() => {
    setAgents([]);
    setError(null);
    if (!projectId || !connected) return undefined;
    const client = maybeClient();
    if (!client) return undefined;
    let cancelled = false;
    setLoading(true);
    client
      .call("agents.list", { projectId })
      .then((r) => {
        if (!cancelled) setAgents(r.agents);
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(errorMessage(e));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    const off = client.subscribe([topic.project(projectId)], (ev: Event) => {
      if (AGENT_EVENTS.has(ev.kind)) setAgents((prev) => applyAgentEvent(prev, ev));
    });
    return () => {
      cancelled = true;
      off();
    };
  }, [projectId, connected, nonce]);

  return { agents, loading, error, reload };
}

export interface AgentTranscript {
  transcript: TranscriptState;
  loading: boolean;
  error: string | null;
}

const RELOAD_ON = new Set(["ToolCallFinished", "AgentStopped", "AgentStateChanged"]);

export function useAgentTranscript(agentId: string | null): AgentTranscript {
  const connected = useAppStore((s) => s.connection.state === "open");
  const [transcript, setTranscript] = useState<TranscriptState>(EMPTY_TRANSCRIPT);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const timer = useRef<number | null>(null);

  useEffect(() => {
    setTranscript(EMPTY_TRANSCRIPT);
    setError(null);
    if (!agentId || !connected) return undefined;
    const client = maybeClient();
    if (!client) return undefined;
    let cancelled = false;
    const load = (initial: boolean) => {
      if (initial) setLoading(true);
      client
        .call("agents.transcript", { agentId })
        .then((r) => {
          if (!cancelled) setTranscript((t) => mergeTranscript(t, r.messages, r.toolCalls));
        })
        .catch((e: unknown) => {
          if (!cancelled) setError(errorMessage(e));
        })
        .finally(() => {
          if (!cancelled && initial) setLoading(false);
        });
    };
    const schedule = () => {
      if (timer.current !== null) window.clearTimeout(timer.current);
      timer.current = window.setTimeout(() => load(false), 250);
    };
    load(true);
    const off = client.subscribe([topic.agent(agentId)], (ev: Event) => {
      setTranscript((t) => applyTranscriptEvent(t, ev));
      if (RELOAD_ON.has(ev.kind) || (ev.kind === "AgentMessage" && ev.payload.done)) schedule();
    });
    return () => {
      cancelled = true;
      off();
      if (timer.current !== null) window.clearTimeout(timer.current);
    };
  }, [agentId, connected]);

  return { transcript, loading, error };
}

export interface Templates {
  templates: AgentTemplate[];
  loading: boolean;
  error: string | null;
  reload: () => void;
}

export function useTemplates(): Templates {
  const connected = useAppStore((s) => s.connection.state === "open");
  const [templates, setTemplates] = useState<AgentTemplate[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [nonce, setNonce] = useState(0);
  const reload = useCallback(() => setNonce((n) => n + 1), []);

  useEffect(() => {
    if (!connected) return undefined;
    const client = maybeClient();
    if (!client) return undefined;
    let cancelled = false;
    setLoading(true);
    client
      .call("templates.list", {})
      .then((r) => {
        if (!cancelled) {
          setTemplates(r.templates);
          setError(null);
        }
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(errorMessage(e));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [connected, nonce]);

  return { templates, loading, error, reload };
}

/** RPC mutations (results also arrive as events). */
export const agentActions = {
  spawn: (params: SpawnAgentParams) => getClient().call("agents.spawn", params),
  stop: (agentId: string, mode?: AgentStopMode) =>
    getClient()
      .call("agents.stop", mode ? { agentId, mode } : { agentId })
      .then((r) => r.agent),
  pause: (agentId: string) => getClient().call("agents.pause", { agentId }).then((r) => r.agent),
  resume: (agentId: string) => getClient().call("agents.resume", { agentId }).then((r) => r.agent),
  handoffs: (agentId: string) => getClient().call("agents.handoff", { agentId }).then((r) => r.handoffs),
};

export const templateActions = {
  create: (template: TemplateDraft) => getClient().call("templates.create", { template }).then((r) => r.template),
  update: (templateId: string, template: TemplateDraft) =>
    getClient().call("templates.update", { templateId, template }).then((r) => r.template),
  remove: (templateId: string) => getClient().call("templates.delete", { templateId }),
  exportJson: (templateId: string) => getClient().call("templates.export", { templateId }),
  importJson: (json: string, acceptPermissions: boolean) =>
    getClient().call("templates.import", { json, acceptPermissions }),
};

/** Saves text as a file in the browser/webview (export). */
export function downloadText(fileName: string, text: string): void {
  const blob = new Blob([text], { type: "application/json" });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = fileName;
  document.body.appendChild(a);
  a.click();
  a.remove();
  window.setTimeout(() => URL.revokeObjectURL(url), 1000);
}
