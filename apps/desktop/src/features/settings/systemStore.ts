/**
 * Node inventory (`nodes.info`), live resources (`nodes.resources` + the
 * `NodeStatus` heartbeat on `node:<id>`) and environment checks
 * (`system.checks`). Shared by Settings → System and onboarding steps 7–8.
 */
import type { NodeInfo, ResourceSample, SystemChecksResult, SystemInfo } from "@orkestr/protocol";
import { topic } from "@orkestr/protocol";
import { create } from "zustand";

import { getClient } from "../../core/client";
import { describeError } from "../../i18n/errors";

export interface SystemState {
  system: SystemInfo | null;
  info: NodeInfo | null;
  infoLoading: boolean;
  infoError: string | null;
  checks: SystemChecksResult | null;
  checksLoading: boolean;
  checksError: string | null;
  sample: ResourceSample | null;
  loadSystem: () => Promise<void>;
  loadInfo: (refresh?: boolean) => Promise<void>;
  runChecks: () => Promise<void>;
  /** Starts streaming live resources; returns the stop function. */
  watchResources: () => () => void;
}

const msg = (e: unknown) => describeError(e);

export const useSystemStore = create<SystemState>()((set, get) => ({
  system: null,
  info: null,
  infoLoading: false,
  infoError: null,
  checks: null,
  checksLoading: false,
  checksError: null,
  sample: null,

  loadSystem: async () => {
    try {
      set({ system: await getClient().call("system.info", {}) });
    } catch (e) {
      console.warn("system.info failed", e);
    }
  },

  loadInfo: async (refresh = false) => {
    if (get().infoLoading) return;
    set({ infoLoading: true, infoError: null });
    try {
      const info = await getClient().call("nodes.info", refresh ? { refresh: true } : {}, { timeoutMs: 120_000 });
      set({ info, infoLoading: false });
    } catch (e) {
      set({ infoLoading: false, infoError: msg(e) });
    }
  },

  runChecks: async () => {
    if (get().checksLoading) return;
    set({ checksLoading: true, checksError: null });
    try {
      const checks = await getClient().call("system.checks", {}, { timeoutMs: 60_000 });
      set({ checks, checksLoading: false });
    } catch (e) {
      set({ checksLoading: false, checksError: msg(e) });
    }
  },

  watchResources: () => {
    let stopped = false;
    let unsubscribe: (() => void) | null = null;
    const client = getClient();
    client.call("nodes.resources", {}).then(
      (r) => {
        if (stopped) return;
        set({ sample: r.sample });
        unsubscribe = client.subscribe([topic.node(r.sample.nodeId)], (ev) => {
          if (ev.kind === "NodeStatus" && ev.payload.sample) set({ sample: ev.payload.sample });
        });
      },
      (e: unknown) => console.warn("nodes.resources failed", e),
    );
    return () => {
      stopped = true;
      unsubscribe?.();
    };
  },
}));
