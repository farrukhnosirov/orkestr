/**
 * Claude Code panel store: status (binary, login), chat tabs per project,
 * the folded chat of every session (fed by `claude:<sessionId>` events),
 * queued messages and the composer insert bus ("Ask Claude about the
 * selection"). All data comes from the core (`claudeCode.*`).
 */
import type {
  ClaudeChatMeta,
  ClaudeCodeEvent,
  ClaudeCodeStatus,
  ClaudeContextUsage,
  ClaudeHistoryEntry,
  ClaudeImage,
  ClaudeMcpServer,
  ClaudePlanUsage,
  ClaudePermissionDecision,
  ClaudePermissionMode,
  ClaudeSessionInfo,
  Event,
} from "@orkestr/protocol";
import { topic } from "@orkestr/protocol";
import { create } from "zustand";

import { getClient, maybeClient } from "../../core/client";
import { t } from "../../i18n";
import { errorMessage } from "../../lib/rpcQuery";
import { applyAll, applyEvent, emptyChat, type ChatState } from "./model";

export interface SessionView {
  info: ClaudeSessionInfo;
  chat: ChatState;
  /** Messages typed while Claude was busy, sent one by one after each turn. */
  queue: string[];
  error: string | null;
  /** Last activity (ISO), for the chat list. */
  updatedAt: string | null;
  /** Extra folders the session was started with (`--add-dir`). */
  addDirs: string[];
  /** Extended thinking switched in this session (null = the CLI's default). */
  thinking: boolean | null;
}

export interface ClaudePrefs {
  model: string;
  mode: ClaudePermissionMode;
  /** Fallback model ("" = none). */
  fallbackModel: string;
  /** The chat list sidebar is open. */
  sidebar: boolean;
}

const PREFS_KEY = "orkestr.claudeCode.prefs";
const DEFAULT_PREFS: ClaudePrefs = { model: "default", mode: "default", fallbackModel: "", sidebar: true };

function readPrefs(): ClaudePrefs {
  try {
    const raw = localStorage.getItem(PREFS_KEY);
    if (!raw) return DEFAULT_PREFS;
    const p = JSON.parse(raw) as Partial<ClaudePrefs>;
    const mode: ClaudePermissionMode = p.mode === "acceptEdits" || p.mode === "plan" ? p.mode : "default";
    return {
      model: typeof p.model === "string" && p.model ? p.model : "default",
      mode,
      fallbackModel: typeof p.fallbackModel === "string" ? p.fallbackModel : "",
      sidebar: typeof p.sidebar === "boolean" ? p.sidebar : true,
    };
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
  /** Text to insert into the composer (selection, edit & resend), consumed by the panel. */
  insert: { text: string; nonce: number; replace: boolean } | null;
  busy: boolean;
  error: string | null;
  /** Orkestr's chat labels by CLI conversation id. */
  meta: Record<string, ClaudeChatMeta>;
  /** Plan limits (session / weekly). */
  usage: ClaudePlanUsage | null;
  usageLoading: boolean;
  usageError: string | null;
  /** Context window usage per session. */
  context: Record<string, ClaudeContextUsage>;
  /** MCP servers per session. */
  mcp: Record<string, ClaudeMcpServer[]>;

  loadStatus: (refresh?: boolean) => Promise<void>;
  setBinaryPath: (path: string | null) => Promise<boolean>;
  loadHistory: (projectId: string) => Promise<void>;
  start: (
    projectId: string,
    opts?: { resume?: string; confirmBypass?: boolean; mode?: ClaudePermissionMode; addDirs?: string[]; cwd?: string },
  ) => Promise<string | null>;
  send: (sessionId: string, text: string, images?: ClaudeImage[]) => Promise<boolean>;
  /** Opens a chat of the list: activates its tab or resumes the conversation. */
  openChat: (projectId: string, entry: { sessionId: string | null; claudeSessionId: string | null }) => Promise<void>;
  setChatMeta: (claudeSessionId: string, patch: { title?: string; pinned?: boolean; hidden?: boolean }) => Promise<boolean>;
  loadUsage: (sessionId?: string | null, refresh?: boolean) => Promise<void>;
  loadContext: (sessionId: string) => Promise<ClaudeContextUsage | null>;
  loadMcp: (sessionId: string) => Promise<ClaudeMcpServer[] | null>;
  setThinking: (sessionId: string, enabled: boolean) => Promise<boolean>;
  /** Runs the interactive `claude` (resuming the chat) in an Orkestr terminal; returns the terminal id. */
  openInTerminal: (projectId: string, claudeSessionId: string | null, cwd?: string) => Promise<string | null>;
  /** Restarts a chat with extra folders (resuming the conversation). */
  restartWithDirs: (projectId: string, sessionId: string, addDirs: string[]) => Promise<string | null>;
  dequeue: (sessionId: string, index: number) => void;
  interrupt: (sessionId: string) => Promise<void>;
  /** Rewinds the chat to before a user message (optionally restoring files); the message goes back into the composer. Resolves the restored files, or null on failure. */
  rewind: (sessionId: string, itemId: string, restoreFiles: boolean) => Promise<string[] | null>;
  setMode: (sessionId: string, mode: ClaudePermissionMode, confirmBypass?: boolean) => Promise<boolean>;
  setModel: (sessionId: string, model: string) => Promise<boolean>;
  respond: (sessionId: string, requestId: string, decision: ClaudePermissionDecision, message?: string) => Promise<void>;
  close: (projectId: string, sessionId: string) => Promise<void>;
  activate: (projectId: string, sessionId: string) => void;
  setPrefs: (patch: Partial<ClaudePrefs>) => void;
  login: (projectId: string | null) => Promise<string | null>;
  requestInsert: (text: string, replace?: boolean) => void;
  consumeInsert: () => string | null;
  /** Applies a core event (exported for tests). */
  onEvent: (ev: Event) => void;
}

const unsubs = new Map<string, () => void>();

let usageTimer: ReturnType<typeof setTimeout> | null = null;
/** Refreshes the plan usage shortly after a turn ends (coalesced). */
function scheduleUsage(sessionId: string): void {
  if (usageTimer) clearTimeout(usageTimer);
  usageTimer = setTimeout(() => {
    usageTimer = null;
    void useClaudeStore.getState().loadUsage(sessionId, true);
  }, 1500);
}

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

  const addSession = (info: ClaudeSessionInfo, history: ClaudeCodeEvent[] = [], addDirs: string[] = []) => {
    const chat = history.length > 0 ? applyAll(emptyChat(), history) : emptyChat();
    set((st) => {
      const tabs = st.tabs[info.projectId] ?? [];
      const view: SessionView = { info, chat, queue: [], error: null, updatedAt: info.startedAt, addDirs, thinking: null };
      return {
        sessions: { ...st.sessions, [info.sessionId]: view },
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
    meta: {},
    usage: null,
    usageLoading: false,
    usageError: null,
    context: {},
    mcp: {},

    loadStatus: async (refresh = false) => {
      set({ statusLoading: true, statusError: null });
      try {
        const status = await getClient().call("claudeCode.status", refresh ? { refresh: true } : {});
        set({ status, statusLoading: false });
        // Re-attach live sessions after a reload of the UI (with their models / commands).
        for (const info of status.sessions) {
          if (!get().sessions[info.sessionId] && info.state !== "exited" && info.state !== "failed") {
            addSession(info);
            const client = getClient();
            const cid = info.claudeSessionId;
            void Promise.all([
              client.call("claudeCode.capabilities", { sessionId: info.sessionId }).catch(() => null),
              cid
                ? client.call("claudeCode.transcript", { projectId: info.projectId, claudeSessionId: cid, cwd: info.cwd }).catch(() => null)
                : Promise.resolve(null),
            ]).then(([caps, transcript]) => {
              patchSession(info.sessionId, (v) => {
                let chat = transcript && transcript.events.length > 0 && v.chat.items.length === 0 ? applyAll(v.chat, transcript.events) : v.chat;
                if (caps?.event) chat = applyEvent(chat, caps.event);
                return { ...v, chat };
              });
            });
          }
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
        const meta: Record<string, ClaudeChatMeta> = {};
        for (const m of res.meta ?? []) meta[m.claudeSessionId] = m;
        set((st) => ({ history: { ...st.history, [projectId]: res.history }, historyLoading: false, meta }));
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
          ...(opts.cwd ? { cwd: opts.cwd } : {}),
          ...(opts.addDirs && opts.addDirs.length > 0 ? { addDirs: opts.addDirs } : {}),
          ...(prefs.fallbackModel ? { fallbackModel: prefs.fallbackModel } : {}),
        });
        addSession(res.session, res.history, opts.addDirs ?? []);
        set({ busy: false });
        return res.session.sessionId;
      } catch (e) {
        set({ busy: false, error: errorMessage(e) });
        return null;
      }
    },

    send: async (sessionId, text, images = []) => {
      const s = get().sessions[sessionId];
      const body = text.trim();
      if (!s || !body) return false;
      if (s.chat.running && images.length > 0) {
        patchSession(sessionId, (v) => ({ ...v, error: t("claudeCodePanel:errors.imagesWhileBusy") }));
        return false;
      }
      if (s.chat.running) {
        patchSession(sessionId, (v) => ({ ...v, queue: [...v.queue, body] }));
        return true;
      }
      // Busy from now on, so a second queued message waits for this turn.
      patchSession(sessionId, (v) => ({ ...v, chat: { ...v.chat, running: true } }));
      try {
        const info = await getClient().call("claudeCode.send", {
          sessionId,
          text: body,
          ...(images.length > 0 ? { images } : {}),
        });
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
        const info = await getClient().call("claudeCode.interrupt", { sessionId });
        patchSession(sessionId, (v) => ({ ...v, info, queue: [] }));
      } catch (e) {
        patchSession(sessionId, (v) => ({ ...v, error: errorMessage(e) }));
      }
    },

    rewind: async (sessionId, itemId, restoreFiles) => {
      const s = get().sessions[sessionId];
      const index = s ? s.chat.items.findIndex((it) => it.id === itemId) : -1;
      const item = s?.chat.items[index];
      if (!s || !item || item.kind !== "user") return null;
      const turnsFromEnd = s.chat.items.slice(index + 1).filter((it) => it.kind === "user").length;
      try {
        const res = await getClient().call("claudeCode.rewind", { sessionId, turnsFromEnd, text: item.text, restoreFiles });
        patchSession(sessionId, (v) => ({
          ...v,
          info: res.session,
          queue: [],
          error: null,
          chat: { ...v.chat, items: v.chat.items.slice(0, index), running: false, thinking: false, streamingId: null, turnFiles: [] },
        }));
        get().requestInsert(res.text, true);
        return res.filesChanged;
      } catch (e) {
        patchSession(sessionId, (v) => ({ ...v, error: errorMessage(e) }));
        return null;
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

    requestInsert: (text, replace = false) => set((st) => ({ insert: { text, nonce: (st.insert?.nonce ?? 0) + 1, replace } })),

    openChat: async (projectId, entry) => {
      if (entry.sessionId && get().sessions[entry.sessionId]) {
        const s = get().sessions[entry.sessionId];
        const ended = s?.info.state === "exited" || s?.info.state === "failed";
        if (!ended || !entry.claudeSessionId) {
          get().activate(projectId, entry.sessionId);
          return;
        }
      }
      if (!entry.claudeSessionId) return;
      // Already open in another tab?
      const open = Object.values(get().sessions).find(
        (v) =>
          v.info.projectId === projectId &&
          (v.chat.claudeSessionId ?? v.info.claudeSessionId) === entry.claudeSessionId &&
          v.info.state !== "exited" &&
          v.info.state !== "failed",
      );
      if (open) {
        get().activate(projectId, open.info.sessionId);
        return;
      }
      await get().start(projectId, { resume: entry.claudeSessionId });
    },

    setChatMeta: async (claudeSessionId, patch) => {
      try {
        const m = await getClient().call("claudeCode.setChatMeta", { claudeSessionId, ...patch });
        set((st) => ({ meta: { ...st.meta, [claudeSessionId]: m } }));
        return true;
      } catch (e) {
        set({ error: errorMessage(e) });
        return false;
      }
    },

    loadUsage: async (sessionId, refresh = false) => {
      if (get().usageLoading) return;
      set({ usageLoading: true });
      try {
        const usage = await getClient().call("claudeCode.usage", {
          ...(sessionId ? { sessionId } : {}),
          ...(refresh ? { refresh: true } : {}),
        });
        set({ usage, usageLoading: false, usageError: null });
      } catch (e) {
        set({ usageLoading: false, usageError: errorMessage(e) });
      }
    },

    loadContext: async (sessionId) => {
      try {
        const c = await getClient().call("claudeCode.contextUsage", { sessionId });
        set((st) => ({ context: { ...st.context, [sessionId]: c } }));
        return c;
      } catch (e) {
        patchSession(sessionId, (v) => ({ ...v, error: errorMessage(e) }));
        return null;
      }
    },

    loadMcp: async (sessionId) => {
      try {
        const r = await getClient().call("claudeCode.mcpStatus", { sessionId });
        set((st) => ({ mcp: { ...st.mcp, [sessionId]: r.servers } }));
        return r.servers;
      } catch (e) {
        patchSession(sessionId, (v) => ({ ...v, error: errorMessage(e) }));
        return null;
      }
    },

    setThinking: async (sessionId, enabled) => {
      try {
        const info = await getClient().call("claudeCode.setThinking", { sessionId, enabled });
        patchSession(sessionId, (v) => ({ ...v, info, thinking: enabled, error: null }));
        return true;
      } catch (e) {
        patchSession(sessionId, (v) => ({ ...v, error: errorMessage(e) }));
        return false;
      }
    },

    openInTerminal: async (projectId, claudeSessionId, cwd) => {
      try {
        const res = await getClient().call("claudeCode.openInTerminal", {
          projectId,
          ...(claudeSessionId ? { claudeSessionId } : {}),
          ...(cwd ? { cwd } : {}),
        });
        return res.terminal.terminalId;
      } catch (e) {
        set({ error: errorMessage(e) });
        return null;
      }
    },

    restartWithDirs: async (projectId, sessionId, addDirs) => {
      const s = get().sessions[sessionId];
      if (!s) return null;
      const resume = s.chat.claudeSessionId ?? s.info.claudeSessionId ?? undefined;
      const cwd = s.info.cwd;
      await get().close(projectId, sessionId);
      return get().start(projectId, { ...(resume ? { resume } : {}), cwd, addDirs });
    },
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
        patchSession(sessionId, (v) => ({
          ...v,
          chat: applyEvent(v.chat, event),
          updatedAt: event.type === "result" || event.type === "userMessage" ? new Date().toISOString() : v.updatedAt,
        }));
        if (event.type === "result") {
          flushQueue(sessionId);
          scheduleUsage(sessionId);
        }
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
