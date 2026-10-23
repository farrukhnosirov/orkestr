/**
 * Codex panel store: the CLI's status (binary, update, ChatGPT sign-in),
 * plan limits, models, saved threads, the folded chat of every live session
 * (fed by `codex:<sessionId>` events), queued messages and preferences.
 * All data comes from the core (`codex.*`).
 */
import type {
  CodexAccountPayload,
  CodexApprovalDecision,
  CodexApprovalMode,
  CodexEvent,
  CodexLimits,
  CodexLoginMode,
  CodexLoginState,
  CodexModelOption,
  CodexSessionInfo,
  CodexStatus,
  CodexThread,
  Event,
} from "@orkestr/protocol";
import { TOPIC_GLOBAL } from "@orkestr/protocol";
import { create } from "zustand";

import { getClient, maybeClient } from "../../core/client";
import { errorMessage } from "../../lib/rpcQuery";
import { applyCodexAll, applyCodexEvent, emptyCodexChat, settleCodexChat, type CodexChat } from "./model";

/**
 * Topic of one Codex session. The core publishes on `codex:<id>`; the TS
 * topic helper (`@orkestr/protocol` `topic`) does not know this scope yet.
 */
export const codexTopic = (sessionId: string): string => `codex:${sessionId}`;

export interface CodexSessionView {
  info: CodexSessionInfo;
  chat: CodexChat;
  /** Messages typed while Codex was busy, sent one by one after each turn. */
  queue: string[];
  error: string | null;
  /** Last activity (ISO). */
  updatedAt: string | null;
}

export interface CodexPrefs {
  /** Model id ("" = the CLI's default). */
  model: string;
  /** Reasoning effort ("" = the model's default). */
  effort: string;
  mode: CodexApprovalMode;
  sidebar: boolean;
}

const PREFS_KEY = "orkestr.codex.prefs";
export const DEFAULT_CODEX_PREFS: CodexPrefs = { model: "", effort: "", mode: "ask", sidebar: true };

function readPrefs(): CodexPrefs {
  try {
    const raw = localStorage.getItem(PREFS_KEY);
    if (!raw) return DEFAULT_CODEX_PREFS;
    const p = JSON.parse(raw) as Partial<CodexPrefs>;
    return {
      model: typeof p.model === "string" ? p.model : "",
      effort: typeof p.effort === "string" ? p.effort : "",
      mode: p.mode === "readOnly" || p.mode === "auto" ? p.mode : "ask",
      sidebar: typeof p.sidebar === "boolean" ? p.sidebar : true,
    };
  } catch {
    return DEFAULT_CODEX_PREFS;
  }
}

function writePrefs(p: CodexPrefs): void {
  try {
    localStorage.setItem(PREFS_KEY, JSON.stringify(p));
  } catch {
    // Storage unavailable: preferences are simply not remembered.
  }
}

export interface CodexState {
  status: CodexStatus | null;
  statusLoading: boolean;
  statusError: string | null;
  installing: boolean;
  /** Result of the last install / update (log tail on failure). */
  install: { ok: boolean; log: string } | null;
  /** The sign-in in progress / last finished. */
  login: CodexLoginState | null;
  loginBusy: boolean;
  limits: CodexLimits | null;
  limitsLoading: boolean;
  models: CodexModelOption[];
  modelsFallback: boolean;
  threads: Record<string, CodexThread[]>;
  threadsLoading: boolean;
  threadsError: string | null;
  sessions: Record<string, CodexSessionView>;
  /** Session ids per project (open order). */
  order: Record<string, string[]>;
  active: Record<string, string | null>;
  prefs: CodexPrefs;
  /** Text to put into the composer, consumed by the panel. */
  insert: { text: string; nonce: number } | null;
  busy: boolean;
  error: string | null;

  loadStatus: (refresh?: boolean) => Promise<void>;
  installCli: (update?: boolean) => Promise<boolean>;
  startLogin: (mode: CodexLoginMode) => Promise<void>;
  cancelLogin: () => Promise<void>;
  logout: () => Promise<void>;
  openInTerminal: (opts: { projectId?: string | null; login?: boolean; threadId?: string | null }) => Promise<string | null>;
  loadLimits: (refresh?: boolean) => Promise<void>;
  loadModels: (refresh?: boolean) => Promise<void>;
  loadThreads: (projectId: string) => Promise<void>;
  start: (projectId: string, opts?: { threadId?: string; fork?: boolean }) => Promise<string | null>;
  /** Opens a list entry: activates its live session or resumes the saved thread. */
  openChat: (projectId: string, entry: { sessionId: string | null; threadId: string | null }) => Promise<void>;
  send: (sessionId: string, text: string) => Promise<boolean>;
  dequeue: (sessionId: string, index: number) => void;
  interrupt: (sessionId: string) => Promise<void>;
  compact: (sessionId: string) => Promise<void>;
  setOptions: (sessionId: string | null, patch: { model?: string; effort?: string; mode?: CodexApprovalMode }) => Promise<void>;
  respond: (sessionId: string, requestId: string, decision: CodexApprovalDecision) => Promise<void>;
  close: (projectId: string, sessionId: string) => Promise<void>;
  activate: (projectId: string, sessionId: string) => void;
  setPrefs: (patch: Partial<CodexPrefs>) => void;
  /** Adds a local note to a chat (slash command output). */
  note: (sessionId: string, text: string) => void;
  requestInsert: (text: string) => void;
  consumeInsert: () => string | null;
  /** Applies a core event (exported for tests). */
  onEvent: (ev: Event) => void;
}

const unsubs = new Map<string, () => void>();
let globalUnsub: (() => void) | null = null;

function subscribe(sessionId: string): void {
  if (unsubs.has(sessionId)) return;
  const c = maybeClient();
  if (!c) return;
  try {
    const off = c.subscribe([codexTopic(sessionId)], (ev) => useCodexStore.getState().onEvent(ev));
    unsubs.set(sessionId, off);
  } catch (e) {
    // An older protocol package rejects the `codex:` scope; status still flows on `project:<id>`.
    console.warn("codex: cannot subscribe to the session topic", e);
  }
}

function unsubscribe(sessionId: string): void {
  unsubs.get(sessionId)?.();
  unsubs.delete(sessionId);
}

/** Follows sign-in progress / account changes (`CodexAccount`, global). Idempotent. */
export function watchCodexAccount(): void {
  if (globalUnsub) return;
  const c = maybeClient();
  if (!c) return;
  globalUnsub = c.subscribe([TOPIC_GLOBAL], (ev) => {
    if (ev.kind === "CodexAccount") useCodexStore.getState().onEvent(ev);
  });
}

let limitsTimer: ReturnType<typeof setTimeout> | null = null;
function scheduleLimits(): void {
  if (limitsTimer) clearTimeout(limitsTimer);
  limitsTimer = setTimeout(() => {
    limitsTimer = null;
    void useCodexStore.getState().loadLimits(true);
  }, 1500);
}

const ENDED = new Set(["exited", "failed"]);

export const useCodexStore = create<CodexState>()((set, get) => {
  const patchSession = (id: string, fn: (s: CodexSessionView) => CodexSessionView) =>
    set((st) => {
      const cur = st.sessions[id];
      return cur ? { sessions: { ...st.sessions, [id]: fn(cur) } } : {};
    });

  const flushQueue = (id: string) => {
    const s = get().sessions[id];
    if (!s || s.chat.running || s.queue.length === 0 || ENDED.has(s.info.state)) return;
    const [next, ...rest] = s.queue;
    patchSession(id, (v) => ({ ...v, queue: rest }));
    if (next) void get().send(id, next);
  };

  const addSession = (info: CodexSessionInfo, history: CodexEvent[] = []) => {
    const chat = history.length > 0 ? applyCodexAll(emptyCodexChat(), history) : emptyCodexChat();
    set((st) => {
      const order = st.order[info.projectId] ?? [];
      const view: CodexSessionView = {
        info,
        chat: { ...chat, threadId: chat.threadId ?? info.threadId, model: chat.model ?? info.model, effort: chat.effort ?? info.effort },
        queue: [],
        error: null,
        updatedAt: info.startedAt,
      };
      return {
        sessions: { ...st.sessions, [info.sessionId]: view },
        order: { ...st.order, [info.projectId]: order.includes(info.sessionId) ? order : [...order, info.sessionId] },
        active: { ...st.active, [info.projectId]: info.sessionId },
      };
    });
    subscribe(info.sessionId);
  };

  /** Loads a thread's saved items into a freshly resumed / re-attached session. */
  const loadHistory = async (sessionId: string, threadId: string) => {
    try {
      const res = await getClient().call("codex.transcript", { threadId });
      if (res.events.length === 0) return;
      patchSession(sessionId, (v) => {
        if (v.chat.items.length > 0) return v;
        const past = applyCodexAll(emptyCodexChat(), res.events);
        return { ...v, chat: { ...v.chat, items: past.items, seq: past.seq, tokens: v.chat.tokens ?? past.tokens } };
      });
    } catch {
      // The transcript is a convenience: the chat continues without it.
    }
  };

  return {
    status: null,
    statusLoading: false,
    statusError: null,
    installing: false,
    install: null,
    login: null,
    loginBusy: false,
    limits: null,
    limitsLoading: false,
    models: [],
    modelsFallback: false,
    threads: {},
    threadsLoading: false,
    threadsError: null,
    sessions: {},
    order: {},
    active: {},
    prefs: readPrefs(),
    insert: null,
    busy: false,
    error: null,

    loadStatus: async (refresh = false) => {
      set({ statusLoading: true, statusError: null });
      watchCodexAccount();
      try {
        const status = await getClient().call("codex.status", refresh ? { refresh: true } : {});
        set({ status, statusLoading: false, login: status.login ?? get().login });
        for (const info of status.sessions) {
          if (!get().sessions[info.sessionId] && !ENDED.has(info.state)) {
            addSession(info);
            if (info.threadId) void loadHistory(info.sessionId, info.threadId);
          }
        }
      } catch (e) {
        set({ statusLoading: false, statusError: errorMessage(e) });
      }
    },

    installCli: async (update = false) => {
      set({ installing: true, install: null });
      try {
        const res = await getClient().call("codex.install", update ? { update: true } : {});
        set({ installing: false, install: { ok: res.ok, log: res.log }, status: res.status });
        return res.ok;
      } catch (e) {
        set({ installing: false, install: { ok: false, log: errorMessage(e) } });
        return false;
      }
    },

    startLogin: async (mode) => {
      set({ loginBusy: true, error: null });
      watchCodexAccount();
      try {
        const login = await getClient().call("codex.login", { mode });
        set({ login, loginBusy: false });
      } catch (e) {
        set({ loginBusy: false, error: errorMessage(e) });
      }
    },

    cancelLogin: async () => {
      try {
        const status = await getClient().call("codex.loginCancel", {});
        set({ status, login: status.login });
      } catch (e) {
        set({ error: errorMessage(e) });
      }
    },

    logout: async () => {
      try {
        const status = await getClient().call("codex.logout", {});
        set({ status, login: null, limits: null });
      } catch (e) {
        set({ error: errorMessage(e) });
      }
    },

    openInTerminal: async ({ projectId, login, threadId }) => {
      try {
        const res = await getClient().call("codex.openInTerminal", {
          ...(projectId ? { projectId } : {}),
          ...(login ? { login: true } : {}),
          ...(threadId ? { threadId } : {}),
        });
        return res.terminal.terminalId;
      } catch (e) {
        set({ error: errorMessage(e) });
        return null;
      }
    },

    loadLimits: async (refresh = false) => {
      if (get().limitsLoading) return;
      set({ limitsLoading: true });
      try {
        const limits = await getClient().call("codex.limits", refresh ? { refresh: true } : {});
        set({ limits, limitsLoading: false });
      } catch (e) {
        set({ limitsLoading: false, error: errorMessage(e) });
      }
    },

    loadModels: async (refresh = false) => {
      try {
        const res = await getClient().call("codex.models", refresh ? { refresh: true } : {});
        set({ models: res.models, modelsFallback: res.fallback });
      } catch (e) {
        set({ error: errorMessage(e) });
      }
    },

    loadThreads: async (projectId) => {
      set({ threadsLoading: true });
      try {
        const res = await getClient().call("codex.threads", { projectId });
        set((st) => ({ threads: { ...st.threads, [projectId]: res.threads }, threadsLoading: false, threadsError: res.error }));
        for (const info of res.live) {
          if (!get().sessions[info.sessionId] && !ENDED.has(info.state)) addSession(info);
        }
      } catch (e) {
        set({ threadsLoading: false, threadsError: errorMessage(e) });
      }
    },

    start: async (projectId, opts = {}) => {
      const { prefs } = get();
      set({ busy: true, error: null });
      try {
        const info = await getClient().call("codex.start", {
          projectId,
          ...(opts.threadId ? { threadId: opts.threadId } : {}),
          ...(opts.threadId && opts.fork ? { fork: true } : {}),
          ...(prefs.model ? { model: prefs.model } : {}),
          ...(prefs.effort ? { effort: prefs.effort } : {}),
          mode: prefs.mode,
        });
        addSession(info);
        set({ busy: false });
        if (opts.threadId) void loadHistory(info.sessionId, opts.threadId);
        return info.sessionId;
      } catch (e) {
        set({ busy: false, error: errorMessage(e) });
        return null;
      }
    },

    openChat: async (projectId, entry) => {
      if (entry.sessionId) {
        const s = get().sessions[entry.sessionId];
        if (s && (!ENDED.has(s.info.state) || !entry.threadId)) {
          get().activate(projectId, entry.sessionId);
          return;
        }
      }
      if (!entry.threadId) return;
      const open = Object.values(get().sessions).find(
        (v) => v.info.projectId === projectId && (v.chat.threadId ?? v.info.threadId) === entry.threadId && !ENDED.has(v.info.state),
      );
      if (open) {
        get().activate(projectId, open.info.sessionId);
        return;
      }
      await get().start(projectId, { threadId: entry.threadId });
    },

    send: async (sessionId, text) => {
      const s = get().sessions[sessionId];
      const body = text.trim();
      if (!s || !body) return false;
      if (s.chat.running) {
        patchSession(sessionId, (v) => ({ ...v, queue: [...v.queue, body] }));
        return true;
      }
      patchSession(sessionId, (v) => ({ ...v, chat: { ...v.chat, running: true } }));
      try {
        const info = await getClient().call("codex.send", { sessionId, text: body });
        patchSession(sessionId, (v) => ({ ...v, info, error: null, updatedAt: new Date().toISOString() }));
        return true;
      } catch (e) {
        patchSession(sessionId, (v) => ({ ...v, chat: { ...v.chat, running: false }, error: errorMessage(e) }));
        return false;
      }
    },

    dequeue: (sessionId, index) => patchSession(sessionId, (v) => ({ ...v, queue: v.queue.filter((_, i) => i !== index) })),

    interrupt: async (sessionId) => {
      try {
        const info = await getClient().call("codex.interrupt", { sessionId });
        patchSession(sessionId, (v) => ({ ...v, info, queue: [] }));
      } catch (e) {
        patchSession(sessionId, (v) => ({ ...v, error: errorMessage(e) }));
      }
    },

    compact: async (sessionId) => {
      try {
        const info = await getClient().call("codex.compact", { sessionId });
        patchSession(sessionId, (v) => ({ ...v, info, error: null }));
      } catch (e) {
        patchSession(sessionId, (v) => ({ ...v, error: errorMessage(e) }));
      }
    },

    setOptions: async (sessionId, patch) => {
      const prefs: Partial<CodexPrefs> = {};
      if (patch.model !== undefined) prefs.model = patch.model;
      if (patch.effort !== undefined) prefs.effort = patch.effort;
      if (patch.mode !== undefined) prefs.mode = patch.mode;
      get().setPrefs(prefs);
      if (!sessionId) return;
      try {
        const info = await getClient().call("codex.setOptions", {
          sessionId,
          ...(patch.model ? { model: patch.model } : {}),
          ...(patch.effort ? { effort: patch.effort } : {}),
          ...(patch.mode ? { mode: patch.mode } : {}),
        });
        patchSession(sessionId, (v) => ({ ...v, info, error: null }));
      } catch (e) {
        patchSession(sessionId, (v) => ({ ...v, error: errorMessage(e) }));
      }
    },

    respond: async (sessionId, requestId, decision) => {
      try {
        const info = await getClient().call("codex.respondApproval", { sessionId, requestId, decision });
        patchSession(sessionId, (v) => ({ ...v, info, error: null }));
      } catch (e) {
        patchSession(sessionId, (v) => ({ ...v, error: errorMessage(e) }));
      }
    },

    close: async (projectId, sessionId) => {
      const s = get().sessions[sessionId];
      if (s && !ENDED.has(s.info.state)) {
        try {
          await getClient().call("codex.stop", { sessionId });
        } catch {
          // Already gone: closing the chat is all that is left to do.
        }
      }
      unsubscribe(sessionId);
      set((st) => {
        const order = (st.order[projectId] ?? []).filter((id) => id !== sessionId);
        const sessions = { ...st.sessions };
        delete sessions[sessionId];
        const active = st.active[projectId] === sessionId ? (order[order.length - 1] ?? null) : (st.active[projectId] ?? null);
        return { order: { ...st.order, [projectId]: order }, sessions, active: { ...st.active, [projectId]: active } };
      });
    },

    activate: (projectId, sessionId) => set((st) => ({ active: { ...st.active, [projectId]: sessionId } })),

    setPrefs: (patch) => {
      const prefs = { ...get().prefs, ...patch };
      writePrefs(prefs);
      set({ prefs });
    },

    note: (sessionId, text) => patchSession(sessionId, (v) => ({ ...v, chat: applyCodexEvent(v.chat, { type: "info", text }) })),

    requestInsert: (text) => set((st) => ({ insert: { text, nonce: (st.insert?.nonce ?? 0) + 1 } })),

    consumeInsert: () => {
      const ins = get().insert;
      if (!ins) return null;
      set({ insert: null });
      return ins.text;
    },

    onEvent: (ev) => {
      if (ev.kind === "CodexEvent") {
        const { sessionId, event } = ev.payload;
        if (!get().sessions[sessionId]) return;
        patchSession(sessionId, (v) => ({
          ...v,
          chat: applyCodexEvent(v.chat, event),
          updatedAt: event.type === "turnCompleted" || event.type === "userMessage" ? new Date().toISOString() : v.updatedAt,
        }));
        if (event.type === "turnCompleted") {
          flushQueue(sessionId);
          scheduleLimits();
        }
        if (event.type === "rateLimits") set({ limits: event.limits });
        if (event.type === "error" && event.authRequired) {
          set((st) => (st.status ? { status: { ...st.status, loggedIn: false } } : {}));
        }
      } else if (ev.kind === "CodexStatus") {
        const info = ev.payload.session;
        if (!get().sessions[info.sessionId]) return;
        patchSession(info.sessionId, (v) => ({
          ...v,
          info,
          chat: info.state === "idle" || ENDED.has(info.state) ? settleCodexChat(v.chat) : v.chat,
        }));
        if (info.state === "idle") flushQueue(info.sessionId);
      } else if (ev.kind === "CodexAccount") {
        applyAccount(ev.payload);
      }
    },
  };

  function applyAccount(p: CodexAccountPayload): void {
    set((st) => ({
      login: p.login ?? st.login,
      status: st.status ? { ...st.status, loggedIn: p.loggedIn, account: p.account, login: p.login ?? st.status.login } : st.status,
    }));
    if (p.loggedIn) void get().loadLimits(true);
  }
});

/** Running state of a session view (a turn or a pending approval). */
export function isRunning(v: CodexSessionView | undefined): boolean {
  if (!v) return false;
  return v.chat.running || v.info.state === "running" || v.info.state === "waitingApproval";
}

/** The session ended (exited / failed). */
export function isEnded(v: CodexSessionView | undefined): boolean {
  return v ? ENDED.has(v.info.state) : true;
}
