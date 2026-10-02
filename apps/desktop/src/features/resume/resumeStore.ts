/**
 * Session Resume card state (§57). Shown when a project is opened: what
 * happened in it since the user last opened it (or since the previous core
 * session on app start). Degrades gracefully when the summary is unavailable.
 */
import type { ResumeSummary } from "@orkestr/protocol";
import { create } from "zustand";

import { getClient } from "../../core/client";
import { describeError } from "../../i18n/errors";
import { useAppStore } from "../../store/app";

export interface ResumeState {
  projectId: string | null;
  summary: ResumeSummary | null;
  loading: boolean;
  error: string | null;
  visible: boolean;
  show: (projectId: string, since: string | null) => Promise<void>;
  dismiss: () => void;
}

export const useResumeStore = create<ResumeState>()((set, get) => ({
  projectId: null,
  summary: null,
  loading: false,
  error: null,
  visible: false,

  show: async (projectId, since) => {
    set({ projectId, summary: null, loading: true, error: null, visible: true });
    try {
      const params: { projectId: string; since?: string } = { projectId };
      if (since) params.since = since;
      const summary = await getClient().call("system.resumeSummary", params);
      if (get().projectId === projectId) set({ summary, loading: false });
    } catch (e) {
      if (get().projectId === projectId) set({ loading: false, error: describeError(e) });
    }
  },

  dismiss: () => set({ visible: false }),
}));

/**
 * Watches project switches. The `since` window is the project's previous
 * `lastOpenedAt`, captured synchronously before `projects.open` updates it.
 * Returns the unsubscribe function.
 */
export function startResumeWatcher(): () => void {
  let bootShown = false;
  const initial = useAppStore.getState();
  if (initial.projectsLoaded && initial.activeProjectId) {
    bootShown = true;
    void useResumeStore.getState().show(initial.activeProjectId, null);
  }
  return useAppStore.subscribe((state, prev) => {
    const id = state.activeProjectId;
    if (!id) return;
    if (id !== prev.activeProjectId) {
      const before = prev.projects.find((p) => p.id === id) ?? state.projects.find((p) => p.id === id);
      bootShown = true;
      void useResumeStore.getState().show(id, before?.lastOpenedAt ?? null);
    } else if (!bootShown && state.projectsLoaded && !prev.projectsLoaded) {
      // App start with a restored active project: summarize since the previous core session.
      bootShown = true;
      void useResumeStore.getState().show(id, null);
    }
  });
}
