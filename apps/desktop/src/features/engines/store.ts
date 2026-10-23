/**
 * Parallel engines store: both subscriptions' status and the routing
 * configuration (`engines.status` / `engines.setConfig`), shared by the
 * title-bar indicator and the settings card.
 */
import type { EnginesConfig, EnginesStatus } from "@orkestr/protocol";
import { create } from "zustand";

import { getClient } from "../../core/client";
import { errorMessage } from "../../lib/rpcQuery";

export interface EnginesState {
  status: EnginesStatus | null;
  loading: boolean;
  saving: boolean;
  error: string | null;
  load: (refresh?: boolean) => Promise<void>;
  save: (config: EnginesConfig) => Promise<boolean>;
}

export const useEnginesStore = create<EnginesState>()((set, get) => ({
  status: null,
  loading: false,
  saving: false,
  error: null,
  load: async (refresh = false) => {
    if (get().loading) return;
    set({ loading: true });
    try {
      const status = await getClient().call("engines.status", refresh ? { refresh: true } : {});
      set({ status, loading: false, error: null });
    } catch (e) {
      set({ loading: false, error: errorMessage(e) });
    }
  },
  save: async (config) => {
    set({ saving: true, error: null });
    try {
      const status = await getClient().call("engines.setConfig", config);
      set({ status, saving: false });
      return true;
    } catch (e) {
      set({ saving: false, error: errorMessage(e) });
      return false;
    }
  },
}));
