/**
 * Notification center state: history (bell), visible toasts, Do Not Disturb
 * (persisted in core settings as `doNotDisturb`).
 */
import type { NotificationLevel } from "@orkestr/protocol";
import { create } from "zustand";

import type { AppNotification } from "./fromEvent";
import { shouldToast } from "./fromEvent";

export const MAX_HISTORY = 200;
export const MAX_TOASTS = 4;
/** Auto-dismiss delay per level (ms); critical toasts stay until dismissed. */
export const TOAST_TTL: Record<NotificationLevel, number | null> = {
  Info: 5000,
  Success: 4000,
  Warning: 8000,
  Error: 10000,
  Critical: null,
};

export interface NotificationsState {
  items: AppNotification[];
  toasts: AppNotification[];
  doNotDisturb: boolean;
  centerOpen: boolean;
  push: (n: AppNotification) => void;
  /** Convenience for UI-originated notices. */
  notify: (level: NotificationLevel, title: string, body?: string) => void;
  dismissToast: (id: string) => void;
  markAllRead: () => void;
  clear: () => void;
  setDoNotDisturb: (v: boolean) => void;
  setCenterOpen: (v: boolean) => void;
}

let localCounter = 0;

export const useNotificationsStore = create<NotificationsState>()((set, get) => ({
  items: [],
  toasts: [],
  doNotDisturb: false,
  centerOpen: false,

  push: (n) => {
    if (get().items.some((x) => x.id === n.id)) return; // replay / duplicate delivery
    const toast = shouldToast(n, get().doNotDisturb);
    set((s) => ({
      items: [n, ...s.items].slice(0, MAX_HISTORY),
      toasts: toast ? [...s.toasts, n].slice(-MAX_TOASTS) : s.toasts,
    }));
  },

  notify: (level, title, body = "") => {
    localCounter += 1;
    get().push({
      id: `local-${Date.now()}-${localCounter}`,
      ts: new Date().toISOString(),
      level,
      title,
      body,
      projectId: null,
      source: "local",
      read: false,
    });
  },

  dismissToast: (id) => set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })),
  markAllRead: () => set((s) => ({ items: s.items.map((i) => (i.read ? i : { ...i, read: true })) })),
  clear: () => set({ items: [], toasts: [] }),
  setDoNotDisturb: (v) => set((s) => ({ doNotDisturb: v, toasts: v ? s.toasts.filter((t) => t.level === "Critical") : s.toasts })),
  setCenterOpen: (v) => set({ centerOpen: v }),
}));

/** Unread count (selector). */
export function unreadCount(items: readonly AppNotification[]): number {
  return items.reduce((n, i) => (i.read ? n : n + 1), 0);
}
