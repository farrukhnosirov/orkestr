/**
 * AI-editable UI store (§29). Holds the stored UI configuration (as the core
 * reports it), the Manager's pending proposal with its server-side preview,
 * and the undo/redo history. Every change goes through the core:
 *
 *   UiPatchProposed (event) → ui.previewPatch (validated, nothing stored)
 *     → live preview + diff card → Keep: ui.applyPatch → UiConfigPatched
 *                                 → Revert: dropped, nothing stored
 *
 * The UI renders `effectiveConfig()` — the preview while one is shown,
 * otherwise the stored document.
 */
import type {
  PreviewUiPatchResult,
  UiConfig,
  UiConfigPatchedPayload,
  UiPatchActor,
  UiPatchOp,
  UiPatchProposedPayload,
  UiPrefs,
} from "@orkestr/protocol";
import { OrkestrError } from "@orkestr/client";
import { useMemo } from "react";
import { create } from "zustand";

import { maybeClient } from "../../core/client";
import { t } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { checkPatch } from "./patch";
import { prefsOf } from "./prefs";
import {
  EMPTY_UNDO,
  canRedo,
  canUndo,
  completeRedo,
  completeUndo,
  peekRedo,
  peekUndo,
  recordApplied,
  type UndoState,
} from "./undo";

export type ProposalStatus = "previewing" | "applying" | "invalid" | "applied";

export interface Proposal {
  id: string;
  projectId: string | null;
  author: UiPatchActor;
  summary: string;
  patch: UiPatchOp[];
  status: ProposalStatus;
  preview: PreviewUiPatchResult | null;
  error: string | null;
  /** When the card closes by itself (only for `applied`). */
  receivedAt: number;
}

export interface AiUiState {
  config: UiConfig | null;
  version: number;
  loaded: boolean;
  proposal: Proposal | null;
  /** Show the preview (true) or the original (false) while comparing. */
  previewVisible: boolean;
  undo: UndoState;
  busy: boolean;
  error: string | null;

  load: () => Promise<void>;
  receiveProposal: (p: UiPatchProposedPayload) => Promise<void>;
  keepProposal: () => Promise<boolean>;
  discardProposal: () => void;
  setPreviewVisible: (v: boolean) => void;
  /** A user-authored change (settings controls). */
  applyUserPatch: (patch: UiPatchOp[], summary: string) => Promise<boolean>;
  undoLast: () => Promise<boolean>;
  redoLast: () => Promise<boolean>;
  onPatched: (p: UiConfigPatchedPayload) => void;
  closeCard: () => void;
}

let localSeq = 0;

function client() {
  const c = maybeClient();
  return c && c.state === "open" ? c : null;
}

export const useAiUiStore = create<AiUiState>()((set, get) => ({
  config: null,
  version: 0,
  loaded: false,
  proposal: null,
  previewVisible: true,
  undo: EMPTY_UNDO,
  busy: false,
  error: null,

  load: async () => {
    const c = client();
    if (!c) return;
    try {
      // An empty preview returns the stored document and its version.
      const r = await c.call("ui.previewPatch", { patch: [] });
      set({ config: r.base, version: r.version, loaded: true, error: null });
    } catch (e) {
      set({ loaded: true, error: describeError(e) });
    }
  },

  receiveProposal: async (p) => {
    const base: Proposal = {
      id: p.proposalId,
      projectId: p.projectId,
      author: p.author,
      summary: p.summary,
      patch: p.patch,
      status: "previewing",
      preview: null,
      error: null,
      receivedAt: Date.now(),
    };
    const pre = checkPatch(p.patch, p.author);
    if (pre.length > 0) {
      set({ proposal: { ...base, status: "invalid", error: t("aiui:errors.rejected") } });
      return;
    }
    set({ proposal: base, previewVisible: true });
    const c = client();
    if (!c) {
      set({ proposal: { ...base, status: "invalid", error: t("aiui:errors.offline") } });
      return;
    }
    try {
      const preview = await c.call("ui.previewPatch", { patch: p.patch, author: p.author });
      if (get().proposal?.id !== p.proposalId) return; // superseded
      set({ proposal: { ...base, preview }, config: preview.base, version: preview.version });
    } catch (e) {
      if (get().proposal?.id !== p.proposalId) return;
      set({ proposal: { ...base, status: "invalid", error: describeError(e) } });
    }
  },

  keepProposal: async () => {
    const p = get().proposal;
    const c = client();
    if (!p || !p.preview || p.status !== "previewing" || !c) return false;
    set({ proposal: { ...p, status: "applying" } });
    try {
      const r = await c.call("ui.applyPatch", {
        patch: p.patch,
        baseVersion: p.preview.version,
        author: p.author,
        summary: p.summary,
        proposalId: p.id,
      });
      set((s) => ({
        config: r.config,
        version: r.version,
        proposal: { ...p, status: "applied", receivedAt: Date.now() },
        undo: recordApplied(s.undo, { id: p.id, summary: p.summary, author: p.author, forward: p.patch, inverse: r.inverse, at: Date.now() }),
      }));
      return true;
    } catch (e) {
      // A conflict means the document changed meanwhile: preview again.
      if (e instanceof OrkestrError && e.code === "conflict") {
        void get().receiveProposal({ proposalId: p.id, projectId: p.projectId, author: p.author, summary: p.summary, patch: p.patch });
      } else {
        set({ proposal: { ...p, status: "previewing", error: describeError(e) } });
      }
      return false;
    }
  },

  discardProposal: () => set({ proposal: null, previewVisible: true }),
  closeCard: () => set({ proposal: null, previewVisible: true }),
  setPreviewVisible: (previewVisible) => set({ previewVisible }),

  applyUserPatch: async (patch, summary) => {
    const c = client();
    if (!c || checkPatch(patch, "User").length > 0) return false;
    set({ busy: true, error: null });
    try {
      const r = await c.call("ui.applyPatch", { patch, author: "User", summary });
      set((s) => ({
        config: r.config,
        version: r.version,
        busy: false,
        undo: recordApplied(s.undo, { id: `local-${++localSeq}`, summary, author: "User", forward: patch, inverse: r.inverse, at: Date.now() }),
      }));
      return true;
    } catch (e) {
      set({ busy: false, error: describeError(e) });
      return false;
    }
  },

  undoLast: async () => {
    const s = get();
    const top = peekUndo(s.undo);
    const c = client();
    if (!top || !c || !canUndo(s.undo)) return false;
    set({ busy: true, error: null });
    try {
      const r = await c.call("ui.applyPatch", { patch: top.inverse, author: "User", summary: t("aiui:history.undoSummary", { summary: top.summary }) });
      set((st) => ({ config: r.config, version: r.version, busy: false, undo: completeUndo(st.undo, r.inverse) }));
      return true;
    } catch (e) {
      set({ busy: false, error: describeError(e) });
      return false;
    }
  },

  redoLast: async () => {
    const s = get();
    const top = peekRedo(s.undo);
    const c = client();
    if (!top || !c || !canRedo(s.undo)) return false;
    set({ busy: true, error: null });
    try {
      const r = await c.call("ui.applyPatch", { patch: top.forward, author: "User", summary: t("aiui:history.redoSummary", { summary: top.summary }) });
      set((st) => ({ config: r.config, version: r.version, busy: false, undo: completeRedo(st.undo, r.inverse) }));
      return true;
    } catch (e) {
      set({ busy: false, error: describeError(e) });
      return false;
    }
  },

  onPatched: (p) => {
    if (p.version <= get().version) return;
    set({ config: p.config, version: p.version });
  },
}));

/** The document the UI should render now (preview while one is shown). */
export function effectiveConfig(s: Pick<AiUiState, "config" | "proposal" | "previewVisible">): UiConfig | null {
  const p = s.proposal;
  if (p && p.preview && s.previewVisible && (p.status === "previewing" || p.status === "applying")) return p.preview.config;
  return s.config;
}

/** Effective shell preferences (React). Mission Control, HUD etc. read these. */
export function useUiPrefs(): UiPrefs {
  const config = useAiUiStore(effectiveConfig);
  return useMemo(() => prefsOf(config), [config]);
}
