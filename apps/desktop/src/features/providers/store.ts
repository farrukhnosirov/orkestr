/**
 * Provider hub client state. All data comes from the core over RPC
 * (`providers.*`) and `ProviderStatusChanged` events — never from Tauri.
 */
import { OrkestrError } from "@orkestr/client";
import type {
  CreateProviderParams,
  Event,
  ManagerConfig,
  Model,
  ProviderConfigPatch,
  ProviderKindInfo,
  ProviderView,
  RoutingConfig,
  TestProviderResult,
} from "@orkestr/protocol";
import { TOPIC_GLOBAL } from "@orkestr/protocol";
import { useEffect } from "react";
import { create } from "zustand";

import { getClient, maybeClient } from "../../core/client";
import { useAppStore } from "../../store/app";

export interface ProvidersState {
  kinds: ProviderKindInfo[];
  providers: ProviderView[];
  loaded: boolean;
  loading: boolean;
  error: string | null;
  pricingTableDate: string | null;
  setAll: (kinds: ProviderKindInfo[], providers: ProviderView[], pricingTableDate: string) => void;
  upsert: (view: ProviderView) => void;
  remove: (providerId: string) => void;
  setLoading: (loading: boolean, error?: string | null) => void;
}

export const useProvidersStore = create<ProvidersState>((set) => ({
  kinds: [],
  providers: [],
  loaded: false,
  loading: false,
  error: null,
  pricingTableDate: null,
  setAll: (kinds, providers, pricingTableDate) => set({ kinds, providers, pricingTableDate, loaded: true, loading: false, error: null }),
  upsert: (view) =>
    set((s) => {
      const i = s.providers.findIndex((p) => p.provider.id === view.provider.id);
      if (i < 0) return { providers: [...s.providers, view] };
      const next = [...s.providers];
      next[i] = view;
      return { providers: next };
    }),
  remove: (providerId) => set((s) => ({ providers: s.providers.filter((p) => p.provider.id !== providerId) })),
  setLoading: (loading, error = null) => set({ loading, error }),
}));

/** Human message for any thrown value. */
export function errorMessage(e: unknown): string {
  if (e instanceof OrkestrError) return e.message;
  if (e instanceof Error) return e.message;
  return String(e);
}

/** Loads kinds + providers. */
export async function loadProviders(): Promise<void> {
  const store = useProvidersStore.getState();
  store.setLoading(true);
  try {
    const c = getClient();
    const [kinds, list] = await Promise.all([c.call("providers.kinds", {}), c.call("providers.list", {})]);
    useProvidersStore.getState().setAll(kinds.kinds, list.providers, kinds.pricingTableDate);
  } catch (e) {
    useProvidersStore.getState().setLoading(false, errorMessage(e));
  }
}

async function refreshOne(providerId: string): Promise<void> {
  try {
    const { provider } = await getClient().call("providers.get", { providerId });
    useProvidersStore.getState().upsert(provider);
  } catch (e) {
    if (e instanceof OrkestrError && e.code === "not_found") useProvidersStore.getState().remove(providerId);
  }
}

const pending = new Map<string, ReturnType<typeof setTimeout>>();

function onEvent(event: Event): void {
  if (event.kind !== "ProviderStatusChanged" && event.kind !== "ProviderRateLimited") return;
  const id = event.payload.providerId;
  const known = useProvidersStore.getState().providers.some((p) => p.provider.id === id);
  // Debounce bursts (health loop, fallbacks) into one refresh per provider.
  const prev = pending.get(id);
  if (prev) clearTimeout(prev);
  pending.set(
    id,
    setTimeout(() => {
      pending.delete(id);
      if (known) void refreshOne(id);
      else void loadProviders();
    }, 150),
  );
}

/**
 * Loads providers once the core is connected and keeps them live through
 * events. Safe to mount from several components.
 */
export function useProvidersLive(): ProvidersState {
  const connected = useAppStore((s) => s.connection.state === "open");
  const state = useProvidersStore();
  useEffect(() => {
    if (!connected) return undefined;
    const st = useProvidersStore.getState();
    if (!st.loaded && !st.loading) void loadProviders();
    const client = maybeClient();
    if (!client) return undefined;
    return client.subscribe([TOPIC_GLOBAL], onEvent);
  }, [connected]);
  return state;
}

// ── mutations ─────────────────────────────────────────────

export async function createProvider(params: CreateProviderParams): Promise<ProviderView> {
  const { provider } = await getClient().call("providers.create", params);
  useProvidersStore.getState().upsert(provider);
  void checkHealth(provider.provider.id);
  return provider;
}

export async function updateProvider(providerId: string, patch: ProviderConfigPatch): Promise<ProviderView> {
  const { provider } = await getClient().call("providers.update", { providerId, patch });
  useProvidersStore.getState().upsert(provider);
  return provider;
}

export async function deleteProvider(providerId: string): Promise<void> {
  await getClient().call("providers.delete", { providerId });
  useProvidersStore.getState().remove(providerId);
}

export async function setCredential(providerId: string, apiKey: string): Promise<ProviderView> {
  const { provider } = await getClient().call("providers.setCredential", { providerId, apiKey });
  useProvidersStore.getState().upsert(provider);
  void checkHealth(providerId);
  return provider;
}

export async function clearCredential(providerId: string): Promise<ProviderView> {
  const { provider } = await getClient().call("providers.clearCredential", { providerId });
  useProvidersStore.getState().upsert(provider);
  return provider;
}

export async function checkHealth(providerId: string): Promise<void> {
  try {
    await getClient().call("providers.health", { providerId });
  } finally {
    await refreshOne(providerId);
  }
}

export async function listModels(providerId: string, refresh = false): Promise<Model[]> {
  const { models } = await getClient().call("providers.models", refresh ? { providerId, refresh } : { providerId });
  return models;
}

export async function testProvider(providerId: string, model?: string): Promise<TestProviderResult> {
  const result = await getClient().call("providers.test", model ? { providerId, model } : { providerId });
  await refreshOne(providerId);
  return result;
}

export async function getManagerConfig(): Promise<ManagerConfig> {
  return (await getClient().call("providers.getManager", {})).config;
}

export async function saveManagerConfig(config: ManagerConfig): Promise<ManagerConfig> {
  return (await getClient().call("providers.setManager", { config })).config;
}

export async function getRoutingConfig(): Promise<RoutingConfig> {
  return (await getClient().call("providers.getRouting", {})).config;
}

export async function saveRoutingConfig(config: RoutingConfig): Promise<RoutingConfig> {
  return (await getClient().call("providers.setRouting", { config })).config;
}
