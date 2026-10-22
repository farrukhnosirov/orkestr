/** Scriptable `RemoteClient` fake: handlers per method, call log, event emitter. */
import { OrkestrError } from "@orkestr/client";
import type { Event } from "@orkestr/protocol";

type Handler = (params: Record<string, unknown>) => unknown;

export interface FakeClient {
  call: jest.Mock;
  subscribe: jest.Mock;
  calls: { method: string; params: Record<string, unknown> }[];
  emit(ev: Event): void;
}

export function fakeClient(handlers: Record<string, Handler | unknown>): FakeClient {
  const listeners = new Set<(ev: Event) => void>();
  const calls: FakeClient["calls"] = [];
  const call = jest.fn(async (method: string, params: Record<string, unknown>) => {
    calls.push({ method, params });
    const h = handlers[method];
    if (h === undefined) throw new OrkestrError("not_found", `no fake for ${method}`);
    return typeof h === "function" ? (h as Handler)(params) : h;
  });
  const subscribe = jest.fn((_topics: string[], fn: (ev: Event) => void) => {
    listeners.add(fn);
    return () => listeners.delete(fn);
  });
  return { call, subscribe, calls, emit: (ev) => listeners.forEach((l) => l(ev)) };
}

export const stepUpRequired = (challenge = "ch-1") =>
  new OrkestrError("forbidden", "step-up required", { stepUpRequired: true, challenge });

export function approval(over: Record<string, unknown> = {}) {
  return {
    id: "a-1",
    projectId: null,
    taskId: null,
    agentId: null,
    toolCallId: null,
    capability: "Shell",
    actionSummary: "Run `git push --force`",
    risk: "High",
    reason: "dangerous command",
    status: "Pending",
    requestedAt: "2026-10-22T10:00:00Z",
    expiresAt: null,
    resolvedAt: null,
    resolvedBy: null,
    resolutionNote: null,
    commandPreview: "git push --force",
    danger: "Destructive",
    ...over,
  };
}
