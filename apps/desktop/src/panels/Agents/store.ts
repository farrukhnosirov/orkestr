/**
 * View state of the Agents panel (zustand). Business data is NOT stored here —
 * it comes from the core via the hooks in `data.ts`. `activeAgents` mirrors
 * which agents are running so commands (Stop Agent) can be enabled.
 */
import type { AgentTemplate, TemplateDraft } from "@orkestr/protocol";
import { create } from "zustand";

export type AgentsTab = "agents" | "queue" | "templates";

export interface AgentsToast {
  id: number;
  tone: "danger" | "success" | "info";
  message: string;
}

/** Template editor target: a new draft or an existing template. */
export type EditorTarget = { mode: "create"; draft: TemplateDraft } | { mode: "edit"; template: AgentTemplate };

export interface AgentsUiState {
  tab: AgentsTab;
  selectedAgentId: string | null;
  /** Agent ids whose runs are active (kept in sync by the panel). */
  activeAgentIds: string[];
  spawnOpen: boolean;
  /** Template preselected in the spawn dialog. */
  spawnTemplateId: string | null;
  editor: EditorTarget | null;
  importOpen: boolean;
  toasts: AgentsToast[];
  setTab: (tab: AgentsTab) => void;
  select: (id: string | null) => void;
  setActiveAgents: (ids: string[]) => void;
  openSpawn: (templateId?: string | null) => void;
  closeSpawn: () => void;
  setEditor: (e: EditorTarget | null) => void;
  setImportOpen: (open: boolean) => void;
  toast: (tone: AgentsToast["tone"], message: string) => void;
  dismissToast: (id: number) => void;
}

let seq = 0;

export const useAgentsUi = create<AgentsUiState>()((set, get) => ({
  tab: "agents",
  selectedAgentId: null,
  activeAgentIds: [],
  spawnOpen: false,
  spawnTemplateId: null,
  editor: null,
  importOpen: false,
  toasts: [],
  setTab: (tab) => set({ tab }),
  select: (selectedAgentId) => set({ selectedAgentId }),
  setActiveAgents: (ids) => {
    const cur = get().activeAgentIds;
    if (cur.length === ids.length && cur.every((x, i) => x === ids[i])) return;
    set({ activeAgentIds: ids });
  },
  openSpawn: (templateId = null) => set({ spawnOpen: true, spawnTemplateId: templateId, tab: "agents" }),
  closeSpawn: () => set({ spawnOpen: false }),
  setEditor: (editor) => set({ editor }),
  setImportOpen: (importOpen) => set({ importOpen }),
  toast: (tone, message) => {
    const id = ++seq;
    set({ toasts: [...get().toasts, { id, tone, message }].slice(-4) });
    window.setTimeout(() => get().dismissToast(id), tone === "danger" ? 6000 : 3500);
  },
  dismissToast: (id) => set({ toasts: get().toasts.filter((t) => t.id !== id) }),
}));
