/**
 * Claude Code panel store: status (binary, login), chat tabs per project,
 * the folded chat of every session (fed by `claude:<sessionId>` events),
 * queued messages and the composer insert bus ("Ask Claude about the
 * selection"). All data comes from the core (`claudeCode.*`).
 */
import type {
  ClaudeCodeEvent,
  ClaudeCodeStatus,
  ClaudeHistoryEntry,
  ClaudePermissionDecision,
  ClaudePermissionMode,
  ClaudeSessionInfo,
  Event,
} from "@orkestr/protocol";
import { topic } from "@orkestr/protocol";
import { create } from "zustand";

import { getClient, maybeClient } from "../../core/client";
import { errorMessage } from "../../lib/rpcQuery";
import { applyAll, applyEvent, emptyChat, type ChatState } from "./model";

export interface SessionView {
  info: ClaudeSessionInfo;
  chat: ChatState;
  /** Messages typed while Claude was busy, sent one by one after each turn. */
  queue: string[];
  error: string | null;
}

export interface ClaudePrefs {
  model: string;
  mode: ClaudePermissionMode;
}

const PREFS_KEY = "orkestr.claudeCode.prefs";
const DEFAULT_PREFS: ClaudePrefs = { model: "default", mode: "default" };

function readPrefs(): ClaudePrefs {
  try {
    const raw = localStorage.getItem(PREFS_KEY);
    if (!raw) return DEFAULT_PREFS;
    const p = JSON.parse(raw) as Partial<ClaudePrefs>;
    const mode: ClaudePermissionMode = p.mode === "acceptEdits" || p.mode === "plan" ? p.mode : "default";
    return { model: typeof p.model === "string" && p.model ? p.model : "default", mode };
  } catch {
    return DEFAULT_PREFS;
  }
}

function writePrefs(p: ClaudePrefs): void {
  try {
    // bypassPermissions is never remembered: it is confirmed per session.
    localStorage.setItem(PREFS_KEY, JSON.stringify({ ...p, mode: p.mode === "bypassPermissions" ? "default" : p.mode }));
  } catch {
    // Storage unavailable: preferences are simply not remembered.
  }
}

export interface ClaudeState {
  status: ClaudeCodeStatus | null;
  statusLoading: boolean;
  statusError: string | null;
  sessions: Record<string, SessionView>;
  /** Open chat tabs (session ids) per project. */
  tabs: Record<string, string[]>;
  /** Active tab per project. */
  active: Record<string, string | null>;
  history: Record<string, ClaudeHistoryEntry[]>;
  historyLoading: boolean;
  prefs: ClaudePrefs;
  /** Text to insert into the composer (selection), consumed by the panel. */
  insert: { text: string; nonce: number } | null;
  busy: boolean;
  error: string | null;

  loadStatus: (refresh?: boolean) => Promise<void>;
  setBinaryPath: (path: string | null) => Promise<boolean>;
  loadHistory: (projectId: string) => Promise<void>;
  start: (projectId: string, opts?: { resume?: string; confirmBypass?: boolean; mode?: ClaudePermissionMode }) => Promise<string | null>;
  send: (sessionId: string, text: string) => Promise<boolean>;
  dequeue: (sessionId: string, index: number) => void;
  interrupt: (sessionId: string) => Promise<void>;
  setMode: (sessionId: string, mode: ClaudePermissionMode, confirmBypass?: boolean) => Promise<boolean>;
  setModel: (sessionId: string, model: string) => Promise<boolean>;
  respond: (sessionId: string, requestId: string, decision: ClaudePermissionDecision, message?: string) => Promise<void>;
  close: (projectId: string, sessionId: string) => Promise<void>;
  activate: (projectId: string, sessionId: string) => void;
  setPrefs: (patch: Partial<ClaudePrefs>) => void;
  login: (projectId: string | null) => Promise<string | null>;
  requestInsert: (text: string) => void;
  consumeInsert: () => string | null;
  /** Applies a core event (exported for tests). */
  onEvent: (ev: Event) => void;
}

const unsubs = new Map<string, () => void>();

function subscribe(sessionId: string): void {
  if (unsubs.has(sessionId)) return;
  const c = maybeClient();
  if (!c) return;
  const off = c.subscribe([topic.claude(sessionId)], (ev) => useClaudeStore.getState().onEvent(ev));
  unsubs.set(sessionId, off);
}

function unsubscribe(sessionId: string): void {
  unsubs.get(sessionId)?.();
  unsubs.delete(sessionId);
}

export const useClaudeStore = create<ClaudeState>()((set, get) => {
  const patchSession = (id: string, fn: (s: SessionView) => SessionView) =>
    set((st) => {
      const cur = st.sessions[id];
      return cur ? { sessions: { ...st.sessions, [id]: fn(cur) } } : {};
    });

  const flushQueue = (id: string) => {
    const s = get().sessions[id];
    if (!s || s.chat.running || s.queue.length === 0) return;
    if (s.info.state === "exited" || s.info.state === "failed") return;
    const [next, ...rest] = s.queue;
    patchSession(id, (v) => ({ ...v, queue: rest }));
    if (next) void get().send(id, next);
  };

  const addSession = (info: ClaudeSessionInfo, history: ClaudeCodeEvent[] = []) => {
    const chat = history.length > 0 ? applyAll(emptyChat(), history) : emptyChat();
    set((st) => {
      const tabs = st.tabs[info.projectId] ?? [];
      return {
        sessions: { ...st.sessions, [info.sessionId]: { info, chat, queue: [], error: null } },
        tabs: { ...st.tabs, [info.projectId]: tabs.includes(info.sessionId) ? tabs : [...tabs, info.sessionId] },
        active: { ...st.active, [info.projectId]: info.sessionId },
      };
    });
    subscribe(info.sessionId);
  };

  return {
    status: null,
    statusLoading: false,
    statusError: null,
    sessions: {},
    tabs: {},
    active: {},
    history: {},
    historyLoading: false,
    prefs: readPrefs(),
    insert: null,
    busy: false,
    error: null,

    loadStatus: async (refresh = false) => {
      set({ statusLoading: true, statusError: null });
      try {
        const status = await getClient().call("claudeCode.status", refresh ? { refresh: true } : {});
        set({ status, statusLoading: false });
        // Re-attach live sessions after a reload of the UI.
        for (const info of status.sessions) {
          if (!get().sessions[info.sessionId] && info.state !== "exited" && info.state !== "failed") addSession(info);
        }
      } catch (e) {
        set({ statusLoading: false, statusError: errorMessage(e) });
      }
    },

    setBinaryPath: async (path) => {
      try {
        const status = await getClient().call("claudeCode.setBinaryPath", { path: path?.trim() ? path.trim() : null });
        set({ status, statusError: null });
        return true;
      } catch (e) {
        set({ statusError: errorMessage(e) });
        return false;
      }
    },

    loadHistory: async (projectId) => {
      set({ historyLoading: true });
      try {
        const res = await getClient().call("claudeCode.sessions", { projectId });
        set((st) => ({ history: { ...st.history, [projectId]: res.history }, historyLoading: false }));
      } catch (e) {
        set({ historyLoading: false, error: errorMessage(e) });
      }
    },

    start: async (projectId, opts = {}) => {
      const prefs = get().prefs;
      const mode = opts.mode ?? prefs.mode;
      set({ busy: true, error: null });
      try {
        const res = await getClient().call("claudeCode.start", {
          projectId,
          ...(prefs.model !== "default" ? { model: prefs.model } : {}),
          permissionMode: mode,
          ...(mode === "bypassPermissions" ? { confirmBypass: opts.confirmBypass === true } : {}),
          ...(opts.resume ? { resume: opts.resume } : {}),
        });
        addSession(res.session, res.history);
        set({ busy: false });
        return res.session.sessionId;
      } catch (e) {
        set({ busy: false, error: errorMessage(e) });
        return null;
      }
    },

    send: async (sessionId, text) => {
      const s = get().sessions[sessionId];
      const body = text.trim();
      if (!s || !body) return false;
      if (s.chat.running) {
        patchSession(sessionId, (v) => ({ ...v, queue: [...v.queue, body] }));
        return true;
      }
      // Busy from now on, so a second queued message waits for this turn.
      patchSession(sessionId, (v) => ({ ...v, chat: { ...v.chat, running: true } }));
      try {
        const info = await getClient().call("claudeCode.send", { sessionId, text: body });
        patchSession(sessionId, (v) => ({ ...v, info, error: null }));
        return true;
      } catch (e) {
        patchSession(sessionId, (v) => ({ ...v, chat: { ...v.chat, running: false }, error: errorMessage(e) }));
        return false;
      }
    },

    dequeue: (sessionId, index) => patchSession(sessionId, (v) => ({ ...v, queue: v.queue.filter((_, i) => i !== index) })),

    interrupt: async (sessionId) => {
      try {
        const info = await getClient().call("claudeCode.interrupt", { sessionId });
        patchSession(sessionId, (v) => ({ ...v, info, queue: [] }));
      } catch (e) {
        patchSession(sessionId, (v) => ({ ...v, error: errorMessage(e) }));
      }
    },

    setMode: async (sessionId, mode, confirmBypass) => {
      try {
        const info = await getClient().call("claudeCode.setMode", {
          sessionId,
          mode,
          ...(mode === "bypassPermissions" ? { confirmBypass: confirmBypass === true } : {}),
        });
        patchSession(sessionId, (v) => ({ ...v, info, error: null }));
        get().setPrefs({ mode });
        return true;
      } catch (e) {
        patchSession(sessionId, (v) => ({ ...v, error: errorMessage(e) }));
        return false;
      }
    },

    setModel: async (sessionId, model) => {
      try {
        const info = await getClient().call("claudeCode.setModel", { sessionId, model });
        patchSession(sessionId, (v) => ({ ...v, info, error: null }));
        get().setPrefs({ model });
        return true;
      } catch (e) {
        patchSession(sessionId, (v) => ({ ...v, error: errorMessage(e) }));
        return false;
      }
    },

    respond: async (sessionId, requestId, decision, message) => {
      try {
        const info = await getClient().call("claudeCode.respondPermission", {
          sessionId,
          requestId,
          decision,
          ...(message?.trim() ? { message: message.trim() } : {}),
        });
        patchSession(sessionId, (v) => ({ ...v, info, error: null }));
      } catch (e) {
        patchSession(sessionId, (v) => ({ ...v, error: errorMessage(e) }));
      }
    },

    close: async (projectId, sessionId) => {
      const s = get().sessions[sessionId];
      if (s && s.info.state !== "exited" && s.info.state !== "failed") {
        try {
          await getClient().call("claudeCode.stop", { sessionId });
        } catch {
          // Already gone: closing the tab is all that is left to do.
        }
      }
      unsubscribe(sessionId);
      set((st) => {
        const tabs = (st.tabs[projectId] ?? []).filter((id) => id !== sessionId);
        const sessions = { ...st.sessions };
        delete sessions[sessionId];
        const active = st.active[projectId] === sessionId ? (tabs[tabs.length - 1] ?? null) : (st.active[projectId] ?? null);
        return { tabs: { ...st.tabs, [projectId]: tabs }, sessions, active: { ...st.active, [projectId]: active } };
      });
    },

    activate: (projectId, sessionId) => set((st) => ({ active: { ...st.active, [projectId]: sessionId } })),

    setPrefs: (patch) => {
      const prefs = { ...get().prefs, ...patch };
      writePrefs(prefs);
      set({ prefs });
    },

    login: async (projectId) => {
      try {
        const res = await getClient().call("claudeCode.loginInTerminal", projectId ? { projectId } : {});
        return res.terminal.terminalId;
      } catch (e) {
        set({ error: errorMessage(e) });
        return null;
      }
    },

    requestInsert: (text) => set((st) => ({ insert: { text, nonce: (st.insert?.nonce ?? 0) + 1 } })),
    consumeInsert: () => {
      const ins = get().insert;
      if (!ins) return null;
      set({ insert: null });
      return ins.text;
    },

    onEvent: (ev) => {
      if (ev.kind === "ClaudeCodeEvent") {
        const { sessionId, event } = ev.payload;
        if (!get().sessions[sessionId]) return;
        patchSession(sessionId, (v) => ({ ...v, chat: applyEvent(v.chat, event) }));
        if (event.type === "result") flushQueue(sessionId);
        if (event.type === "capabilities" && event.account) {
          // A session knows the login state better than a stale status probe.
          const account = event.account;
          set((st) => (st.status ? { status: { ...st.status, auth: { ...(st.status.auth ?? account), loggedIn: account.loggedIn } } } : {}));
        }
      } else if (ev.kind === "ClaudeCodeStatus") {
        const info = ev.payload.session;
        patchSession(info.sessionId, (v) => ({ ...v, info }));
        if (info.state === "idle") flushQueue(info.sessionId);
      }
    },
  };
});
