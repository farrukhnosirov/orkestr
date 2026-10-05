/**
 * Review / consensus data over RPC (`reviews.*`, `consensus.*`) plus live
 * updates from `ReviewUpdated` and `ConsensusUpdated` events. Business data
 * comes only from the core through `@orkestr/client`.
 */
import type {
  AdversarialReviewParams,
  ConsensusRun,
  FindingStatus,
  RequestReviewParams,
  Review,
  ReviewGate,
  RunConsensusParams,
} from "@orkestr/protocol";
import { topic } from "@orkestr/protocol";
import { useCallback, useEffect, useState } from "react";

import { getClient, maybeClient } from "../../core/client";
import { describeError } from "../../i18n/errors";
import { useAppStore } from "../../store/app";
import { upsertReview, upsertRun } from "./model";

export interface TaskReviews {
  reviews: Review[];
  gate: ReviewGate | null;
  loading: boolean;
  error: string | null;
  reload: () => void;
}

/** Reviews of one task (newest first) + its quality-gate view, kept live. */
export function useTaskReviews(taskId: string): TaskReviews {
  const connected = useAppStore((s) => s.connection.state === "open");
  const [reviews, setReviews] = useState<Review[]>([]);
  const [gate, setGate] = useState<ReviewGate | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [nonce, setNonce] = useState(0);
  const reload = useCallback(() => setNonce((n) => n + 1), []);

  useEffect(() => {
    setReviews([]);
    setGate(null);
    setError(null);
    const client = maybeClient();
    if (!connected || !client) return undefined;
    let cancelled = false;
    let gateTimer: number | null = null;
    const load = (initial: boolean) =>
      client
        .call("reviews.list", { taskId, limit: 50 })
        .then((r) => {
          if (cancelled) return;
          setReviews(r.reviews);
          setGate(r.gate);
          setError(null);
        })
        .catch((e: unknown) => {
          // A failed background refresh keeps the last good data.
          if (!cancelled && initial) setError(describeError(e));
        })
        .finally(() => {
          if (!cancelled && initial) setLoading(false);
        });
    const loadGate = () => void load(false);

    setLoading(true);
    void load(true);

    const off = client.subscribe([topic.task(taskId)], (ev) => {
      if (ev.kind !== "ReviewUpdated") return;
      const review = ev.payload.review;
      if (review.taskId !== taskId) return;
      setReviews((prev) => upsertReview(prev, review));
      // The gate is computed by the core; refresh it (debounced) after a change.
      if (gateTimer !== null) window.clearTimeout(gateTimer);
      gateTimer = window.setTimeout(loadGate, 150);
    });
    return () => {
      cancelled = true;
      off();
      if (gateTimer !== null) window.clearTimeout(gateTimer);
    };
  }, [taskId, connected, nonce]);

  return { reviews, gate, loading, error, reload };
}

export interface ProjectConsensus {
  runs: ConsensusRun[];
  loading: boolean;
  error: string | null;
  reload: () => void;
}

/** Consensus runs of a project (newest first), kept live. */
export function useConsensusRuns(projectId: string | null): ProjectConsensus {
  const connected = useAppStore((s) => s.connection.state === "open");
  const [runs, setRuns] = useState<ConsensusRun[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [nonce, setNonce] = useState(0);
  const reload = useCallback(() => setNonce((n) => n + 1), []);

  useEffect(() => {
    setRuns([]);
    setError(null);
    const client = maybeClient();
    if (!projectId || !connected || !client) return undefined;
    let cancelled = false;
    setLoading(true);
    client
      .call("consensus.list", { projectId, limit: 100 })
      .then((r) => {
        if (!cancelled) setRuns(r.runs);
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(describeError(e));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    const off = client.subscribe([topic.project(projectId)], (ev) => {
      if (ev.kind !== "ConsensusUpdated" || ev.payload.run.projectId !== projectId) return;
      const run = ev.payload.run;
      setRuns((prev) => upsertRun(prev, run));
    });
    return () => {
      cancelled = true;
      off();
    };
  }, [projectId, connected, nonce]);

  return { runs, loading, error, reload };
}

/** RPC mutations (results also arrive as events). */
export const reviewActions = {
  request: (params: RequestReviewParams) => getClient().call("reviews.request", params),
  adversarial: (params: AdversarialReviewParams) => getClient().call("reviews.adversarial", params),
  resolve: (reviewId: string, findingId: string, status: FindingStatus, note?: string) =>
    getClient().call("reviews.resolveFinding", note ? { reviewId, findingId, status, note } : { reviewId, findingId, status }),
  runConsensus: (params: RunConsensusParams) => getClient().call("consensus.run", params).then((r) => r.run),
  getConsensus: (consensusId: string) => getClient().call("consensus.get", { consensusId }).then((r) => r.run),
};
