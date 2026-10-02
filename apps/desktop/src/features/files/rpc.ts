/**
 * `files.*` client helpers: error decoding, ref-counted watchers that
 * survive reconnects, and a hook for `FileChanged` events of a project.
 */
import { OrkestrError } from "@orkestr/client";
import type { EventOf, FileChangedPayload } from "@orkestr/protocol";
import { topic } from "@orkestr/protocol";
import { useEffect, useRef } from "react";

import { maybeClient } from "../../core/client";
import { useAppStore } from "../../store/app";

/** Decoded RPC error for UI branching. */
export interface ErrorInfo {
  code: string;
  message: string;
  /** `data.reason` from the core (`stale`, `exists`, `too_large`, `recursive_required`). */
  reason: string | null;
  data: Record<string, unknown> | null;
}

/** Decodes any thrown value. */
export function errorInfo(e: unknown): ErrorInfo {
  if (e instanceof OrkestrError) {
    const data = e.data !== null && typeof e.data === "object" ? (e.data as Record<string, unknown>) : null;
    const reason = data && typeof data.reason === "string" ? data.reason : null;
    return { code: e.code, message: e.message, reason, data };
  }
  return { code: "internal", message: e instanceof Error ? e.message : String(e), reason: null, data: null };
}

/** Human message for an error. */
export function errorMessage(e: unknown): string {
  return errorInfo(e).message;
}

// ── Ref-counted watchers ──────────────────────────────────
//
// `files.watch` is idempotent per core session and a session's watchers die
// with it, so the client only has to (re)send `files.watch` for every held
// project whenever a connection opens, and `files.unwatch` when the last
// holder releases while connected.

const watchCounts = new Map<string, number>();

function sendWatch(projectId: string): void {
  const client = maybeClient();
  if (client?.state === "open") client.call("files.watch", { projectId }).catch(() => undefined);
}

/** Re-sends `files.watch` for every held project (call when a connection opens). */
export function syncWatches(): void {
  for (const [projectId, n] of watchCounts) if (n > 0) sendWatch(projectId);
}

/** Keeps the core watching `projectId` while held. Returns the release function. */
export function retainWatch(projectId: string): () => void {
  const n = (watchCounts.get(projectId) ?? 0) + 1;
  watchCounts.set(projectId, n);
  if (n === 1) sendWatch(projectId);
  let released = false;
  return () => {
    if (released) return;
    released = true;
    const left = (watchCounts.get(projectId) ?? 1) - 1;
    if (left > 0) {
      watchCounts.set(projectId, left);
      return;
    }
    watchCounts.delete(projectId);
    const client = maybeClient();
    if (client?.state === "open") client.call("files.unwatch", { projectId }).catch(() => undefined);
  };
}

/** Number of holders (tests/diagnostics). */
export function watchHolders(projectId: string): number {
  return watchCounts.get(projectId) ?? 0;
}

/**
 * Subscribes to `FileChanged` events of a project while connected and keeps
 * the project watched. The handler may change between renders.
 */
export function useFileEvents(
  projectId: string | null,
  handler: (change: FileChangedPayload, event: EventOf<"FileChanged">) => void,
): void {
  const connected = useAppStore((s) => s.connection.state === "open");
  const ref = useRef(handler);
  ref.current = handler;

  useEffect(() => {
    if (!projectId) return;
    return retainWatch(projectId);
  }, [projectId]);

  useEffect(() => {
    if (!projectId || !connected) return;
    const client = maybeClient();
    if (!client) return;
    syncWatches();
    return client.subscribe([topic.project(projectId)], (ev) => {
      if (ev.kind === "FileChanged") ref.current(ev.payload, ev as EventOf<"FileChanged">);
    });
  }, [projectId, connected]);
}
