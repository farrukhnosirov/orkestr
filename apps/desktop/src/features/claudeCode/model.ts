/**
 * Claude Code chat model (pure): folds the streamed `ClaudeCodeEvent`s of a
 * session into renderable chat items. Unit-tested without React.
 *
 * - Streamed text (`textDelta`) builds an open assistant item; the complete
 *   block (`text`) replaces it, so a missing partial never loses text.
 * - Items of a sub-agent (`parentToolUseId`) nest inside that tool's card.
 * - `thinking` only toggles an indicator — reasoning content is never kept.
 */
import type {
  ClaudeCodeEvent,
  ClaudeModelOption,
  ClaudePermissionDecision,
  ClaudeRisk,
  ClaudeSlashCommand,
  ClaudeAuthInfo,
  ClaudeUsage,
  JsonValue,
} from "@orkestr/protocol";

export type PermissionStatus = "pending" | ClaudePermissionDecision;

export interface UserItem {
  kind: "user";
  id: string;
  text: string;
}
export interface AssistantItem {
  kind: "assistant";
  id: string;
  text: string;
  streaming: boolean;
  synthetic: boolean;
}
export interface ToolItem {
  kind: "tool";
  id: string;
  name: string;
  input: JsonValue;
  result: { content: string; isError: boolean } | null;
  children: ChatItem[];
}
export interface PermissionItem {
  kind: "permission";
  id: string;
  toolName: string;
  toolUseId: string | null;
  input: JsonValue;
  description: string | null;
  blockedPath: string | null;
  risk: ClaudeRisk | null;
  canRemember: boolean;
  status: PermissionStatus;
  byPolicy: boolean;
  message: string | null;
}
export interface ResultItem {
  kind: "result";
  id: string;
  isError: boolean;
  subtype: string;
  text: string | null;
  durationMs: number | null;
  numTurns: number | null;
  costUsd: number | null;
  usage: ClaudeUsage;
  files: string[];
}
export interface ErrorItem {
  kind: "error";
  id: string;
  message: string;
  authRequired: boolean;
}
export type NoticeKind = "interrupted" | "modeChanged" | "modelChanged" | "exited" | "commandOutput";
export interface NoticeItem {
  kind: "notice";
  id: string;
  notice: NoticeKind;
  text: string;
}
export type ChatItem = UserItem | AssistantItem | ToolItem | PermissionItem | ResultItem | ErrorItem | NoticeItem;

export interface ChatState {
  items: ChatItem[];
  /** Claude is thinking (indicator only). */
  thinking: boolean;
  /** A turn is in progress. */
  running: boolean;
  /** Id of the open streamed assistant item. */
  streamingId: string | null;
  /** Models / slash commands / account from the CLI. */
  models: ClaudeModelOption[];
  commands: ClaudeSlashCommand[];
  account: ClaudeAuthInfo | null;
  /** The CLI's conversation id. */
  claudeSessionId: string | null;
  /** Reported model. */
  model: string | null;
  /** Permission mode (CLI spelling). */
  mode: string | null;
  /** Files written in the current turn. */
  turnFiles: string[];
  /** The CLI said the user must sign in. */
  authRequired: boolean;
  rateLimit: { status: string; resetsAt: number | null; window: string | null; utilization: number | null } | null;
  seq: number;
}

export function emptyChat(): ChatState {
  return {
    items: [],
    thinking: false,
    running: false,
    streamingId: null,
    models: [],
    commands: [],
    account: null,
    claudeSessionId: null,
    model: null,
    mode: null,
    turnFiles: [],
    authRequired: false,
    rateLimit: null,
    seq: 0,
  };
}

/** Most items kept per session (oldest are dropped). */
export const MAX_ITEMS = 2000;

function nextId(s: ChatState, prefix: string): [string, number] {
  const seq = s.seq + 1;
  return [`${prefix}-${seq}`, seq];
}

/** Appends `item`, nesting it under `parent` (a tool id) when that tool exists. */
function append(items: ChatItem[], item: ChatItem, parent: string | null): ChatItem[] {
  if (parent) {
    let found = false;
    const nest = (list: ChatItem[]): ChatItem[] =>
      list.map((it) => {
        if (found || it.kind !== "tool") return it;
        if (it.id === parent) {
          found = true;
          return { ...it, children: [...it.children, item] };
        }
        const children = nest(it.children);
        return children === it.children ? it : { ...it, children };
      });
    const nested = nest(items);
    if (found) return nested;
  }
  const out = [...items, item];
  return out.length > MAX_ITEMS ? out.slice(out.length - MAX_ITEMS) : out;
}

/** Applies `fn` to the item with `id` (searching nested tool cards). */
function update(items: ChatItem[], id: string, fn: (it: ChatItem) => ChatItem): ChatItem[] {
  let changed = false;
  const walk = (list: ChatItem[]): ChatItem[] =>
    list.map((it) => {
      if (it.id === id) {
        changed = true;
        return fn(it);
      }
      if (it.kind === "tool" && it.children.length > 0) {
        const children = walk(it.children);
        return children === it.children ? it : { ...it, children };
      }
      return it;
    });
  const out = walk(items);
  return changed ? out : items;
}

/** Closes the streamed assistant item (if any). */
function closeStream(s: ChatState): ChatState {
  if (!s.streamingId) return s;
  const id = s.streamingId;
  return { ...s, streamingId: null, items: update(s.items, id, (it) => (it.kind === "assistant" ? { ...it, streaming: false } : it)) };
}

/** Folds one event into the chat state. */
export function applyEvent(state: ChatState, ev: ClaudeCodeEvent): ChatState {
  let s = state;
  switch (ev.type) {
    case "capabilities":
      return {
        ...s,
        models: ev.models,
        commands: ev.commands,
        account: ev.account,
        authRequired: ev.account ? !ev.account.loggedIn : s.authRequired,
      };
    case "init":
      return { ...s, claudeSessionId: ev.claudeSessionId, model: ev.model ?? s.model, mode: ev.permissionMode ?? s.mode };
    case "userMessage": {
      s = closeStream(s);
      const [id, seq] = nextId(s, "u");
      return { ...s, seq, running: true, turnFiles: [], items: append(s.items, { kind: "user", id, text: ev.text }, null) };
    }
    case "thinking":
      return { ...s, thinking: true, running: true };
    case "textDelta": {
      if (ev.parentToolUseId) return s; // sub-agent partials: wait for the complete block
      if (s.streamingId) {
        const id = s.streamingId;
        return {
          ...s,
          thinking: false,
          items: update(s.items, id, (it) => (it.kind === "assistant" ? { ...it, text: it.text + ev.text } : it)),
        };
      }
      const [id, seq] = nextId(s, "a");
      const item: AssistantItem = { kind: "assistant", id, text: ev.text, streaming: true, synthetic: false };
      return { ...s, seq, thinking: false, running: true, streamingId: id, items: append(s.items, item, null) };
    }
    case "text": {
      if (!ev.parentToolUseId && s.streamingId) {
        const id = s.streamingId;
        return {
          ...s,
          thinking: false,
          streamingId: null,
          items: update(s.items, id, (it) =>
            it.kind === "assistant" ? { ...it, text: ev.text, streaming: false, synthetic: ev.synthetic } : it,
          ),
        };
      }
      const [id, seq] = nextId(s, "a");
      const item: AssistantItem = { kind: "assistant", id, text: ev.text, streaming: false, synthetic: ev.synthetic };
      return { ...s, seq, thinking: false, items: append(s.items, item, ev.parentToolUseId) };
    }
    case "toolUse": {
      s = closeStream(s);
      const item: ToolItem = { kind: "tool", id: ev.id, name: ev.name, input: ev.input, result: null, children: [] };
      return { ...s, thinking: false, running: true, items: append(s.items, item, ev.parentToolUseId) };
    }
    case "toolResult":
      return {
        ...s,
        items: update(s.items, ev.toolUseId, (it) =>
          it.kind === "tool" ? { ...it, result: { content: ev.content, isError: ev.isError } } : it,
        ),
      };
    case "permissionRequest": {
      s = closeStream(s);
      const item: PermissionItem = {
        kind: "permission",
        id: ev.requestId,
        toolName: ev.toolName,
        toolUseId: ev.toolUseId,
        input: ev.input,
        description: ev.description,
        blockedPath: ev.blockedPath,
        risk: ev.risk,
        canRemember: ev.canRemember,
        status: "pending",
        byPolicy: false,
        message: null,
      };
      return { ...s, thinking: false, items: append(s.items, item, null) };
    }
    case "permissionResolved":
      return {
        ...s,
        items: update(s.items, ev.requestId, (it) =>
          it.kind === "permission" ? { ...it, status: ev.decision, byPolicy: ev.byPolicy, message: ev.message } : it,
        ),
      };
    case "modeChanged": {
      if (ev.mode === s.mode) return s;
      const [id, seq] = nextId(s, "n");
      const first = s.mode === null;
      const items = first ? s.items : append(s.items, { kind: "notice", id, notice: "modeChanged", text: ev.mode }, null);
      return { ...s, seq, mode: ev.mode, items };
    }
    case "modelChanged": {
      const [id, seq] = nextId(s, "n");
      return { ...s, seq, model: ev.model, items: append(s.items, { kind: "notice", id, notice: "modelChanged", text: ev.model }, null) };
    }
    case "commandOutput": {
      const [id, seq] = nextId(s, "n");
      return { ...s, seq, items: append(s.items, { kind: "notice", id, notice: "commandOutput", text: ev.text }, null) };
    }
    case "fileChanged":
      return s.turnFiles.includes(ev.path) ? s : { ...s, turnFiles: [...s.turnFiles, ev.path] };
    case "result": {
      s = closeStream(s);
      const [id, seq] = nextId(s, "r");
      const item: ResultItem = {
        kind: "result",
        id,
        isError: ev.isError,
        subtype: ev.subtype,
        text: ev.result,
        durationMs: ev.durationMs,
        numTurns: ev.numTurns,
        costUsd: ev.totalCostUsd,
        usage: ev.usage,
        files: s.turnFiles,
      };
      return { ...s, seq, thinking: false, running: false, turnFiles: [], items: append(s.items, item, null) };
    }
    case "rateLimit":
      return { ...s, rateLimit: { status: ev.status, resetsAt: ev.resetsAt, window: ev.window, utilization: ev.utilization } };
    case "error": {
      s = closeStream(s);
      const [id, seq] = nextId(s, "e");
      const item: ErrorItem = { kind: "error", id, message: ev.message, authRequired: ev.authRequired };
      return { ...s, seq, thinking: false, authRequired: s.authRequired || ev.authRequired, items: append(s.items, item, null) };
    }
    case "interrupted": {
      s = closeStream(s);
      const [id, seq] = nextId(s, "n");
      return { ...s, seq, thinking: false, items: append(s.items, { kind: "notice", id, notice: "interrupted", text: "" }, null) };
    }
    case "exited": {
      s = closeStream(s);
      const [id, seq] = nextId(s, "n");
      const items = append(s.items, { kind: "notice", id, notice: "exited", text: ev.code == null ? "" : String(ev.code) }, null);
      // Pending permission cards can no longer be answered.
      const settled = items.map((it) => (it.kind === "permission" && it.status === "pending" ? { ...it, status: "deny" as const, byPolicy: true } : it));
      return { ...s, seq, thinking: false, running: false, items: settled };
    }
  }
}

/** Folds a list of events (resumed transcripts). */
export function applyAll(state: ChatState, events: readonly ClaudeCodeEvent[]): ChatState {
  let s = state;
  for (const ev of events) s = applyEvent(s, ev);
  return { ...closeStream(s), running: false, thinking: false };
}

// ── helpers ───────────────────────────────────────────────────────────────

/** Normalized path for comparisons (forward slashes, no trailing slash). */
function norm(p: string): string {
  return p.replace(/\\/g, "/").replace(/\/+$/, "");
}

/**
 * Project-relative path of `path` (absolute or relative to `cwd`), or null
 * when it lies outside `root`. Case-insensitive on Windows-style paths.
 */
export function relativeTo(root: string | null | undefined, path: string, cwd?: string | null): string | null {
  if (!root) return null;
  const r = norm(root);
  let p = norm(path);
  const absolute = /^([a-zA-Z]:\/|\/)/.test(p);
  if (!absolute) {
    if (!cwd) return p.replace(/^\.\//, "");
    p = `${norm(cwd)}/${p.replace(/^\.\//, "")}`;
  }
  const windows = /^[a-zA-Z]:\//.test(r);
  const a = windows ? p.toLowerCase() : p;
  const b = windows ? r.toLowerCase() : r;
  if (a === b) return "";
  if (!a.startsWith(`${b}/`)) return null;
  return p.slice(r.length + 1);
}

function str(v: JsonValue | undefined): string | null {
  return typeof v === "string" ? v : null;
}

/** Field of a JSON object input. */
export function field(input: JsonValue, key: string): JsonValue | undefined {
  return input !== null && typeof input === "object" && !Array.isArray(input) ? input[key] : undefined;
}

/** The main argument of a tool call for its card header. */
export function toolSubject(name: string, input: JsonValue): string {
  const pick = (...keys: string[]) => {
    for (const k of keys) {
      const v = str(field(input, k));
      if (v) return v;
    }
    return "";
  };
  switch (name) {
    case "Bash":
    case "PowerShell":
      return pick("command");
    case "Read":
    case "Write":
    case "Edit":
    case "MultiEdit":
      return pick("file_path");
    case "NotebookEdit":
      return pick("notebook_path");
    case "Glob":
    case "Grep":
      return pick("pattern");
    case "WebFetch":
      return pick("url");
    case "WebSearch":
      return pick("query");
    case "Task":
    case "Agent":
      return pick("description", "subagent_type");
    default:
      return pick("description", "file_path", "path", "command", "url", "query", "pattern");
  }
}

/** One line of a simple line diff. */
export interface DiffLine {
  type: "same" | "del" | "add";
  text: string;
}

/** Line diff (LCS; inputs are capped) of an edit, for the text fallback view. */
export function lineDiff(before: string, after: string, maxLines = 400): DiffLine[] {
  const a = before.split("\n").slice(0, maxLines);
  const b = after.split("\n").slice(0, maxLines);
  const n = a.length;
  const m = b.length;
  const dp: number[][] = Array.from({ length: n + 1 }, () => new Array<number>(m + 1).fill(0));
  for (let i = n - 1; i >= 0; i--) {
    const row = dp[i] as number[];
    const below = dp[i + 1] as number[];
    for (let j = m - 1; j >= 0; j--) {
      row[j] = a[i] === b[j] ? (below[j + 1] ?? 0) + 1 : Math.max(below[j] ?? 0, row[j + 1] ?? 0);
    }
  }
  const out: DiffLine[] = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (a[i] === b[j]) {
      out.push({ type: "same", text: a[i] ?? "" });
      i++;
      j++;
    } else if ((dp[i + 1]?.[j] ?? 0) >= (dp[i]?.[j + 1] ?? 0)) {
      out.push({ type: "del", text: a[i] ?? "" });
      i++;
    } else {
      out.push({ type: "add", text: b[j] ?? "" });
      j++;
    }
  }
  while (i < n) out.push({ type: "del", text: a[i++] ?? "" });
  while (j < m) out.push({ type: "add", text: b[j++] ?? "" });
  return out;
}

/** `[before, after]` pairs of an Edit / MultiEdit / Write input. */
export function editPairs(name: string, input: JsonValue): Array<[string, string]> {
  if (name === "Write") return [["", str(field(input, "content")) ?? ""]];
  if (name === "Edit") return [[str(field(input, "old_string")) ?? "", str(field(input, "new_string")) ?? ""]];
  if (name === "MultiEdit") {
    const edits = field(input, "edits");
    if (!Array.isArray(edits)) return [];
    return edits.map((e) => [str(field(e, "old_string")) ?? "", str(field(e, "new_string")) ?? ""]);
  }
  return [];
}

export interface Todo {
  content: string;
  status: "pending" | "in_progress" | "completed";
}

/** TodoWrite items. */
export function todos(input: JsonValue): Todo[] {
  const list = field(input, "todos");
  if (!Array.isArray(list)) return [];
  return list.flatMap((t) => {
    const content = str(field(t, "content"));
    const status = str(field(t, "status"));
    if (!content) return [];
    return [{ content, status: status === "completed" || status === "in_progress" ? status : "pending" }];
  });
}

export type ComposerPopup = { kind: "slash"; query: string } | { kind: "mention"; query: string; start: number } | null;

/** Which completion popup the composer shows for `text` with the caret at `caret`. */
export function composerPopup(text: string, caret: number): ComposerPopup {
  const before = text.slice(0, caret);
  if (/^\/[\w:-]*$/.test(before)) return { kind: "slash", query: before.slice(1) };
  const m = /(^|\s)@([^\s@]*)$/.exec(before);
  if (m) return { kind: "mention", query: m[2] ?? "", start: before.length - (m[2] ?? "").length - 1 };
  return null;
}

/** Inserts a file mention at the popup position. */
export function insertMention(text: string, start: number, caret: number, path: string): { text: string; caret: number } {
  const token = `@${path.includes(" ") ? `"${path}"` : path} `;
  const next = text.slice(0, start) + token + text.slice(caret);
  return { text: next, caret: start + token.length };
}

/** Key action of the composer (Enter sends, Shift+Enter newline, Esc interrupts). */
export function composerKey(e: { key: string; shiftKey: boolean; isComposing?: boolean }): "send" | "interrupt" | null {
  if (e.isComposing) return null;
  if (e.key === "Enter" && !e.shiftKey) return "send";
  if (e.key === "Escape") return "interrupt";
  return null;
}

/** The prompt sent for "Ask Claude about the selection". */
export function selectionPrompt(sel: { path: string; startLine: number; endLine: number; text: string; language?: string | null }): string {
  const range = sel.startLine === sel.endLine ? `${sel.startLine}` : `${sel.startLine}-${sel.endLine}`;
  const fence = sel.text.includes("```") ? "````" : "```";
  return `@${sel.path}:${range}\n${fence}${sel.language ?? ""}\n${sel.text}\n${fence}\n`;
}

/** Total input tokens of a turn (uncached + cache reads + cache writes). */
export function inputTokens(u: ClaudeUsage): number {
  return u.inputTokens + u.cacheReadTokens + u.cacheWriteTokens;
}
