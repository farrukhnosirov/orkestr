/**
 * Chat list of the Claude Code sidebar (pure; unit-tested): merges the open
 * chat tabs (live sessions) with past conversations of the project and
 * Orkestr's own labels (rename / pin / hide), searches and groups them by
 * day: pinned → today → yesterday → this week → earlier.
 */
import type { ClaudeChatMeta, ClaudeHistoryEntry, ClaudeSessionInfo } from "@orkestr/protocol";

export type ChatGroup = "pinned" | "today" | "yesterday" | "week" | "earlier";

export const CHAT_GROUPS: readonly ChatGroup[] = ["pinned", "today", "yesterday", "week", "earlier"];

export interface ChatEntry {
  /** Stable key: the CLI conversation id, or `live:<sessionId>` for a brand-new chat. */
  key: string;
  /** The CLI's conversation id (resumable), when known. */
  claudeSessionId: string | null;
  /** Open chat tab (Orkestr session id), if any. */
  sessionId: string | null;
  /** Display title (custom → live → CLI summary → first prompt); null = "New chat". */
  title: string | null;
  /** Whether the title was set by the user in Orkestr. */
  customTitle: boolean;
  /** Folder the conversation runs / ran in. */
  cwd: string | null;
  /** Number of messages (history), when known. */
  messages: number | null;
  /** Last activity (ISO). */
  updatedAt: string;
  pinned: boolean;
  hidden: boolean;
  /** The process works on a turn / waits for a permission. */
  running: boolean;
  /** The tab is open and its process alive. */
  live: boolean;
}

export interface LiveChat {
  info: ClaudeSessionInfo;
  /** The CLI conversation id reported by the stream (may be newer than `info`). */
  claudeSessionId: string | null;
  running: boolean;
  /** Time of the last item, if any (ISO). */
  updatedAt?: string | null;
}

/** Builds the merged list (unsorted order is irrelevant; see `groupChats`). */
export function buildChatList(
  live: readonly LiveChat[],
  history: readonly ClaudeHistoryEntry[],
  meta: Readonly<Record<string, ClaudeChatMeta>>,
): ChatEntry[] {
  const out = new Map<string, ChatEntry>();
  for (const h of history) {
    const m = meta[h.claudeSessionId];
    const custom = m?.title ?? null;
    out.set(h.claudeSessionId, {
      key: h.claudeSessionId,
      claudeSessionId: h.claudeSessionId,
      sessionId: null,
      title: custom ?? h.title ?? h.firstPrompt ?? null,
      customTitle: custom !== null,
      cwd: null,
      messages: h.messages,
      updatedAt: h.updatedAt,
      pinned: m?.pinned ?? false,
      hidden: m?.hidden ?? false,
      running: false,
      live: false,
    });
  }
  for (const l of live) {
    const cid = l.claudeSessionId ?? l.info.claudeSessionId ?? null;
    const key = cid ?? `live:${l.info.sessionId}`;
    const prev = out.get(key);
    const m = cid ? meta[cid] : undefined;
    const custom = m?.title ?? null;
    const ended = l.info.state === "exited" || l.info.state === "failed";
    out.set(key, {
      key,
      claudeSessionId: cid,
      sessionId: l.info.sessionId,
      title: custom ?? prev?.title ?? l.info.title ?? null,
      customTitle: custom !== null,
      cwd: l.info.cwd,
      messages: prev?.messages ?? null,
      updatedAt: latest(prev?.updatedAt, l.updatedAt ?? null, l.info.startedAt),
      pinned: m?.pinned ?? false,
      // An open tab is always listed, even when hidden earlier.
      hidden: false,
      running: l.running && !ended,
      live: !ended,
    });
  }
  return [...out.values()];
}

function latest(...dates: (string | null | undefined)[]): string {
  let best = "";
  let bestT = -Infinity;
  for (const d of dates) {
    if (!d) continue;
    const t = Date.parse(d);
    if (Number.isFinite(t) && t > bestT) {
      bestT = t;
      best = d;
    }
  }
  return best || new Date(0).toISOString();
}

/** Case-insensitive search over title, folder and id (all words must match). */
export function searchChats(entries: readonly ChatEntry[], query: string): ChatEntry[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return [...entries];
  return entries.filter((e) => {
    const hay = `${e.title ?? ""} ${e.cwd ?? ""} ${e.claudeSessionId ?? ""}`.toLowerCase();
    return words.every((w) => hay.includes(w));
  });
}

function startOfDay(d: Date): number {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
}

/** Day group of a date relative to `now` (local time). */
export function chatGroup(updatedAt: string, now: Date): Exclude<ChatGroup, "pinned"> {
  const t = Date.parse(updatedAt);
  if (!Number.isFinite(t)) return "earlier";
  const today = startOfDay(now);
  const day = 86_400_000;
  if (t >= today) return "today";
  if (t >= today - day) return "yesterday";
  if (t >= today - 6 * day) return "week";
  return "earlier";
}

export interface ChatSection {
  group: ChatGroup;
  entries: ChatEntry[];
}

/**
 * Groups entries (hidden ones left out unless `showHidden`): pinned first,
 * then by day; newest first inside a group; empty groups are dropped.
 */
export function groupChats(entries: readonly ChatEntry[], now: Date, showHidden = false): ChatSection[] {
  const byGroup = new Map<ChatGroup, ChatEntry[]>();
  for (const e of entries) {
    if (e.hidden && !showHidden) continue;
    const g: ChatGroup = e.pinned ? "pinned" : chatGroup(e.updatedAt, now);
    const list = byGroup.get(g) ?? [];
    list.push(e);
    byGroup.set(g, list);
  }
  return CHAT_GROUPS.flatMap((group) => {
    const list = byGroup.get(group);
    if (!list || list.length === 0) return [];
    list.sort((a, b) => Date.parse(b.updatedAt) - Date.parse(a.updatedAt));
    return [{ group, entries: list }];
  });
}

/** Last path segment of a folder (for the sidebar's project hint). */
export function folderName(path: string | null): string | null {
  if (!path) return null;
  const parts = path.replace(/[\\/]+$/, "").split(/[\\/]/);
  return parts[parts.length - 1] || path;
}
