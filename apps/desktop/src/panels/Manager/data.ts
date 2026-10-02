/**
 * Manager data: initial loads over RPC (`manager.history`, `manager.status`,
 * `manager.resumeSummary`) plus live `project:<id>` events
 * (`ManagerMessage` deltas, `ManagerMessageAdded`, `ManagerPlanUpdated`,
 * `ManagerRunUpdated`, settings changes). Mutations go through
 * `managerActions`; their results also arrive as events.
 */
import type {
  ApprovePlanResult,
  ChatAttachment,
  Event,
  ManagerChatResult,
  ManagerClearResult,
  ManagerConfig,
  ManagerPlan,
  ManagerRequestOptions,
  ManagerResumeSummary,
  ManagerRun,
  PlanEdits,
} from "@orkestr/protocol";
import { topic } from "@orkestr/protocol";
import { useCallback, useEffect, useState } from "react";

import { getClient, maybeClient } from "../../core/client";
import { currentLanguage, t } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { useAppStore } from "../../store/app";
import { EMPTY_CHAT, applyManagerEvent, indexById, type ChatState } from "./model";

/** Localized message for a failed call (see `i18n/errors.ts`). */
export function errorMessage(e: unknown): string {
  return describeError(e);
}

const RELEVANT = new Set(["ManagerMessage", "ManagerMessageAdded", "ManagerPlanUpdated", "ManagerRunUpdated"]);

export interface ManagerChat {
  state: ChatState;
  loading: boolean;
  error: string | null;
  reload: () => void;
  /** Replaces the Manager config after a model switch (no event for it in this area). */
  setConfig: (config: ManagerConfig) => void;
}

/** Conversation, plans and runs of a project, kept live by events. */
export function useManagerChat(projectId: string | null): ManagerChat {
  const connected = useAppStore((s) => s.connection.state === "open");
  const [state, setState] = useState<ChatState>(EMPTY_CHAT);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [nonce, setNonce] = useState(0);
  const reload = useCallback(() => setNonce((n) => n + 1), []);
  const setConfig = useCallback((config: ManagerConfig) => setState((s) => ({ ...s, config })), []);

  useEffect(() => {
    setState(EMPTY_CHAT);
    setError(null);
    if (!projectId || !connected) return undefined;
    const client = maybeClient();
    if (!client) return undefined;
    let cancelled = false;
    // Events that arrive while the initial load is in flight are buffered
    // and replayed on top of it, so nothing streamed is lost.
    let buffer: Event[] | null = [];
    const off = client.subscribe([topic.project(projectId)], (ev: Event) => {
      if (!RELEVANT.has(ev.kind)) return;
      if (buffer) buffer.push(ev);
      else setState((s) => applyManagerEvent(s, ev));
    });
    setLoading(true);
    Promise.all([client.call("manager.history", { projectId }), client.call("manager.status", { projectId })])
      .then(([h, st]) => {
        if (cancelled) return;
        let next: ChatState = {
          messages: h.messages,
          streaming: [],
          plans: indexById(h.plans),
          runs: indexById(h.activeRun ? [...st.runs.filter((r) => r.id !== h.activeRun?.id), h.activeRun] : st.runs),
          busy: h.busy,
          config: h.config,
        };
        for (const ev of buffer ?? []) next = applyManagerEvent(next, ev);
        buffer = null;
        setState(next);
      })
      .catch((e: unknown) => {
        buffer = null;
        if (!cancelled) setError(errorMessage(e));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
      off();
    };
  }, [projectId, connected, nonce]);

  return { state, loading, error, reload, setConfig };
}

/** "Last session" summary (§57) for the resume banner. */
export function useResumeSummary(projectId: string | null): ManagerResumeSummary | null {
  const connected = useAppStore((s) => s.connection.state === "open");
  const [summary, setSummary] = useState<ManagerResumeSummary | null>(null);
  useEffect(() => {
    setSummary(null);
    if (!projectId || !connected) return undefined;
    const client = maybeClient();
    if (!client) return undefined;
    let cancelled = false;
    client
      .call("manager.resumeSummary", { projectId })
      .then((s) => {
        if (!cancelled) setSummary(s);
      })
      .catch(() => {
        /* the banner is optional; the conversation shows errors */
      });
    return () => {
      cancelled = true;
    };
  }, [projectId, connected]);
  return summary;
}

/** RPC mutations. */
export const managerActions = {
  chat: (projectId: string, text: string, attachment?: ChatAttachment | null, options?: ManagerRequestOptions): Promise<ManagerChatResult> =>
    getClient().call("manager.chat", {
      projectId,
      text,
      ...(attachment ? { attachment } : {}),
      // The Manager (and the agents it starts) reply in the UI language.
      options: { ...options, language: currentLanguage() },
    }),
  clear: (projectId: string): Promise<ManagerClearResult> => getClient().call("manager.clear", { projectId }),
  approvePlan: (planId: string, edits?: PlanEdits): Promise<ApprovePlanResult> =>
    getClient().call("manager.approvePlan", edits ? { planId, edits } : { planId }),
  rejectPlan: (planId: string, reason?: string): Promise<ManagerPlan> =>
    getClient()
      .call("manager.rejectPlan", reason?.trim() ? { planId, reason: reason.trim() } : { planId })
      .then((r) => r.plan),
  pause: (runId: string): Promise<ManagerRun> => getClient().call("manager.pause", { runId }).then((r) => r.run),
  resume: (runId: string): Promise<ManagerRun> => getClient().call("manager.resume", { runId }).then((r) => r.run),
  cancel: (runId: string): Promise<ManagerRun> => getClient().call("manager.cancel", { runId }).then((r) => r.run),
  setModel: async (config: ManagerConfig, model: ManagerConfig["model"]): Promise<ManagerConfig> =>
    (await getClient().call("providers.setManager", { config: { ...config, model } })).config,
  /** Accepts a task waiting for review (→ Completed) or rejects it (→ Failed, the run retries). */
  reviewTask: (taskId: string, accept: boolean) =>
    getClient().call("tasks.transition", {
      taskId,
      to: accept ? "Completed" : "Failed",
      reason: accept ? t("manager:run.acceptedReason") : t("manager:run.rejectedReason"),
    }),
};
