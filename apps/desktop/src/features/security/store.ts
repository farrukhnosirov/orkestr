/**
 * Security feature store: pending approvals (live), emergency flags, UI
 * state of the inbox / emergency confirm / security center, and toasts.
 * All data comes from the core over RPC + event subscriptions.
 */
import { OrkestrError } from "@orkestr/client";
import type { Approval, EmergencyFlagsView, Event } from "@orkestr/protocol";
import { create } from "zustand";

import { maybeClient } from "../../core/client";
import { describeError } from "../../i18n/errors";
import { t } from "../../i18n";
import { approvalSummary } from "../../lib/coreText";
import { useAppStore } from "../../store/app";
import { useSettingsStore } from "../settings/settingsStore";
import { EMPTY_FLAGS, removeApproval, upsertApproval } from "./logic";

export interface Toast {
  id: string;
  title: string;
  body: string;
  tone: "warning" | "danger" | "info" | "success";
  approvalId?: string;
}

export type SecurityTab = "permissions" | "secrets";

export interface SecurityState {
  pending: Approval[];
  pendingLoaded: boolean;
  pendingError: string | null;
  flags: EmergencyFlagsView;
  inboxOpen: boolean;
  confirmStopOpen: boolean;
  /** When set, the emergency dialog confirms stopping only this project (§52 Stop Project). */
  stopProjectId: string | null;
  centerOpen: boolean;
  centerTab: SecurityTab;
  toasts: Toast[];
  setInboxOpen: (open: boolean) => void;
  setConfirmStopOpen: (open: boolean) => void;
  /** Opens the emergency dialog scoped to one project. */
  openStopProject: (projectId: string) => void;
  openCenter: (tab?: SecurityTab) => void;
  closeCenter: () => void;
  pushToast: (t: Omit<Toast, "id">) => void;
  dismissToast: (id: string) => void;
  setPending: (list: Approval[]) => void;
  applyApproval: (a: Approval) => void;
  dropApproval: (id: string) => void;
  setFlags: (f: EmergencyFlagsView) => void;
  setPendingError: (e: string | null) => void;
}

let toastSeq = 0;

/*
 * `approvals.list` snapshots race with live events: the core answers RPCs
 * and forwards events on separate tasks, so an `ApprovalRequested` /
 * `ApprovalResolved` applied while a list call is in flight may predate or
 * postdate the snapshot. Events seen during the latest refresh are journaled
 * and replayed on top of its snapshot (replaying is idempotent), and the
 * answer of a superseded refresh is dropped.
 */
let refreshGen = 0;
let refreshesInFlight = 0;
let journal: ((list: Approval[]) => Approval[])[] = [];

function journalOp(op: (list: Approval[]) => Approval[]): void {
  if (refreshesInFlight > 0) journal.push(op);
}

export const useSecurityStore = create<SecurityState>()((set) => ({
  pending: [],
  pendingLoaded: false,
  pendingError: null,
  flags: EMPTY_FLAGS,
  inboxOpen: false,
  confirmStopOpen: false,
  stopProjectId: null,
  centerOpen: false,
  centerTab: "permissions",
  toasts: [],
  setInboxOpen: (inboxOpen) => set({ inboxOpen }),
  setConfirmStopOpen: (confirmStopOpen) => set({ confirmStopOpen, stopProjectId: null }),
  openStopProject: (stopProjectId) => set({ confirmStopOpen: true, stopProjectId }),
  // The shared Settings view hosts the "security" / "secrets" sections.
  openCenter: (tab) => {
    const t = tab ?? "permissions";
    set({ centerTab: t });
    useSettingsStore.getState().openSettings(t === "secrets" ? "secrets" : "security");
  },
  closeCenter: () => set({ centerOpen: false }),
  pushToast: (t) => set((s) => ({ toasts: [...s.toasts.slice(-3), { ...t, id: `t${++toastSeq}` }] })),
  dismissToast: (id) => set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })),
  setPending: (pending) => set({ pending, pendingLoaded: true, pendingError: null }),
  applyApproval: (a) => {
    journalOp((list) => upsertApproval(list, a));
    set((s) => ({ pending: upsertApproval(s.pending, a) }));
  },
  dropApproval: (id) => {
    journalOp((list) => removeApproval(list, id));
    set((s) => ({ pending: removeApproval(s.pending, id) }));
  },
  setFlags: (flags) => set({ flags }),
  setPendingError: (pendingError) => set({ pendingError, pendingLoaded: true }),
}));

/** User-facing, localized error message. */
export function errorMessage(e: unknown): string {
  return describeError(e);
}

export async function refreshPending(): Promise<void> {
  const c = maybeClient();
  if (!c) return;
  const gen = ++refreshGen;
  journal = [];
  refreshesInFlight += 1;
  try {
    const { approvals } = await c.call("approvals.list", { status: "Pending" });
    if (gen !== refreshGen) return; // a newer refresh owns the list
    const merged = journal.reduce((list, op) => op(list), approvals);
    journal = [];
    useSecurityStore.getState().setPending(merged);
  } catch (e) {
    if (gen === refreshGen) useSecurityStore.getState().setPendingError(errorMessage(e));
  } finally {
    refreshesInFlight -= 1;
    if (refreshesInFlight === 0) journal = [];
  }
}

export async function refreshFlags(): Promise<void> {
  const c = maybeClient();
  if (!c) return;
  try {
    const { flags } = await c.call("emergency.status", {});
    useSecurityStore.getState().setFlags(flags);
  } catch (e) {
    console.warn("emergency.status failed", e);
  }
}

export async function resolveApproval(approvalId: string, decision: "Approve" | "Reject", note?: string): Promise<void> {
  const c = maybeClient();
  if (!c) throw new OrkestrError("disconnected", "not connected to the core");
  const params: { approvalId: string; decision: "Approve" | "Reject"; note?: string } = { approvalId, decision };
  if (note?.trim()) params.note = note.trim();
  try {
    await c.call("approvals.resolve", params);
  } finally {
    // Whatever happened (resolved, conflict, expired) the item leaves the inbox only via fresh data.
    await refreshPending();
  }
}

/** Stop All AI. Purely local in the core. */
export async function stopAllAi(): Promise<{ agentsStopped: number; approvalsCancelled: number }> {
  const c = maybeClient();
  if (!c) throw new OrkestrError("disconnected", "not connected to the core");
  const r = await c.call("emergency.stopAll", {});
  useSecurityStore.getState().setFlags(r.flags);
  await refreshPending();
  return { agentsStopped: r.agentsStopped, approvalsCancelled: r.approvalsCancelled };
}

/** Stop Project (§52): stops one project's agents, Manager runs and checks. Purely local in the core. */
export async function stopProjectAi(projectId: string): Promise<{ agentsStopped: number; approvalsCancelled: number }> {
  const c = maybeClient();
  if (!c) throw new OrkestrError("disconnected", "not connected to the core");
  const r = await c.call("emergency.stopProject", { projectId });
  await refreshPending();
  return { agentsStopped: r.agentsStopped, approvalsCancelled: r.approvalsCancelled };
}

export async function resetEmergency(): Promise<void> {
  const c = maybeClient();
  if (!c) return;
  const r = await c.call("emergency.reset", {});
  useSecurityStore.getState().setFlags(r.flags);
}

function onSecurityEvent(event: Event): void {
  const s = useSecurityStore.getState();
  switch (event.kind) {
    case "ApprovalRequested": {
      const a = event.payload.approval;
      s.applyApproval(a);
      s.pushToast({
        title: t("security:toast.approvalRequested"),
        body: approvalSummary(a.actionSummary),
        tone: a.risk === "Critical" || a.risk === "High" ? "danger" : "warning",
        approvalId: a.id,
      });
      break;
    }
    case "ApprovalResolved":
      s.dropApproval(event.payload.approvalId);
      break;
    case "EmergencyStop":
      void refreshFlags();
      void refreshPending();
      break;
    // Re-arm / flag switches made in another window or client.
    case "EmergencyFlagsChanged":
      s.setFlags(event.payload.flags);
      break;
    default:
      break;
  }
}

/**
 * Subscribes to `global` + every known project's topic (approvals are
 * project-scoped events) and keeps the subscription in sync with the project
 * list. Returns a disposer.
 */
export function initSecurityLive(): () => void {
  const c = maybeClient();
  if (!c) return () => {};
  let unsub: (() => void) | null = null;
  let topicsKey = "";
  const sync = () => {
    const topics = ["global", ...useAppStore.getState().projects.map((p) => `project:${p.id}`)].sort();
    const key = topics.join(",");
    if (key === topicsKey) return;
    topicsKey = key;
    unsub?.();
    unsub = c.subscribe(topics, onSecurityEvent);
  };
  sync();
  const offProjects = useAppStore.subscribe((state, prev) => {
    if (state.projects !== prev.projects) sync();
  });
  const offState = c.onStateChange((state) => {
    if (state === "open") {
      void refreshPending();
      void refreshFlags();
    }
  });
  // Fallback for missed events (sleep, dropped socket): re-read the flags when
  // the window comes back.
  const onVisible = () => {
    if (!document.hidden) void refreshFlags();
  };
  window.addEventListener("focus", onVisible);
  document.addEventListener("visibilitychange", onVisible);
  void refreshPending();
  void refreshFlags();
  return () => {
    window.removeEventListener("focus", onVisible);
    document.removeEventListener("visibilitychange", onVisible);
    offProjects();
    offState();
    unsub?.();
  };
}
