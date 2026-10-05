/**
 * AI Product Manager data: `pm.list` plus live `PmDraftUpdated` events on
 * `project:<id>`. Mutations go through `pmActions` (their results also
 * arrive as events). The Manager composer's "Product Manager mode" uses
 * `pmActions.draft` too.
 */
import type { Event, ManagerRequestOptions, PmApproveResult, PmDraft, PmEpic } from "@orkestr/protocol";
import { topic } from "@orkestr/protocol";
import { useCallback, useEffect, useState } from "react";

import { getClient, maybeClient } from "../../../core/client";
import { currentLanguage } from "../../../i18n";
import { describeError } from "../../../i18n/errors";
import { useAppStore } from "../../../store/app";
import { applyPmEvent } from "./pmModel";

export interface PmDrafts {
  drafts: PmDraft[];
  loading: boolean;
  error: string | null;
  reload: () => void;
}

export function usePmDrafts(projectId: string | null): PmDrafts {
  const connected = useAppStore((s) => s.connection.state === "open");
  const [drafts, setDrafts] = useState<PmDraft[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [nonce, setNonce] = useState(0);
  const reload = useCallback(() => setNonce((n) => n + 1), []);

  useEffect(() => {
    setDrafts([]);
    setError(null);
    if (!projectId || !connected) return undefined;
    const client = maybeClient();
    if (!client) return undefined;
    let cancelled = false;
    let buffer: Event[] | null = [];
    const off = client.subscribe([topic.project(projectId)], (ev: Event) => {
      if (ev.kind !== "PmDraftUpdated") return;
      if (buffer) buffer.push(ev);
      else setDrafts((prev) => applyPmEvent(prev, ev));
    });
    setLoading(true);
    client
      .call("pm.list", { projectId })
      .then((r) => {
        if (cancelled) return;
        let next = r.drafts;
        for (const ev of buffer ?? []) next = applyPmEvent(next, ev);
        buffer = null;
        setDrafts(next);
      })
      .catch((e: unknown) => {
        buffer = null;
        if (!cancelled) setError(describeError(e));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
      off();
    };
  }, [projectId, connected, nonce]);

  return { drafts, loading, error, reload };
}

export const pmActions = {
  /** Idea → structured draft (the PM replies in the UI language). */
  draft: (projectId: string, idea: string, options?: ManagerRequestOptions): Promise<PmDraft> =>
    getClient().call("pm.draft", { projectId, idea, options: { ...options, language: currentLanguage() } }),
  get: (draftId: string): Promise<PmDraft> => getClient().call("pm.get", { draftId }),
  update: (draftId: string, epic: PmEpic): Promise<PmDraft> => getClient().call("pm.update", { draftId, epic }),
  approve: (draftId: string, startRun: boolean, teamId: string | null): Promise<PmApproveResult> =>
    getClient().call("pm.approve", { draftId, startRun, ...(teamId ? { teamId } : {}) }),
  discard: (draftId: string): Promise<PmDraft> => getClient().call("pm.discard", { draftId }),
};
