/**
 * Chat list of the Codex sidebar (pure; unit-tested): merges live panel
 * sessions with the threads Codex saved (`codex.threads`), searches and
 * groups them by day like the Claude Code sidebar (today → yesterday → this
 * week → earlier).
 */
import type { CodexSessionInfo, CodexThread } from "@orkestr/protocol";

import { chatGroup } from "../claudeCode/chatList";

export type CodexChatGroup = "live" | "today" | "yesterday" | "week" | "earlier";
export const CODEX_CHAT_GROUPS: readonly CodexChatGroup[] = ["live", "today", "yesterday", "week", "earlier"];

export interface CodexChatEntry {
  /** Thread id, or `live:<sessionId>` for a thread not started yet. */
  key: string;
  /** Codex thread id (resumable / forkable), when known. */
  threadId: string | null;
  /** Open panel session, if any. */
  sessionId: string | null;
  /** Title (null = "New chat"). */
  title: string | null;
  cwd: string | null;
  model: string | null;
  /** Last activity (ISO). */
  updatedAt: string;
  /** Source thread of a fork. */
  forkedFrom: string | null;
  /** The session works on a turn / waits for an approval. */
  running: boolean;
  /** An open, alive session. */
  live: boolean;
}

export interface CodexLiveChat {
  info: CodexSessionInfo;
  /** Thread id reported by the stream (may be newer than `info`). */
  threadId: string | null;
  running: boolean;
  updatedAt?: string | null;
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

/** Merges saved threads and live sessions (a live session wins over its saved thread). */
export function buildCodexChatList(live: readonly CodexLiveChat[], threads: readonly CodexThread[]): CodexChatEntry[] {
  const out = new Map<string, CodexChatEntry>();
  for (const th of threads) {
    out.set(th.id, {
      key: th.id,
      threadId: th.id,
      sessionId: null,
      title: th.title.trim() || null,
      cwd: th.cwd,
      model: th.model,
      updatedAt: latest(th.updatedAt, th.createdAt),
      forkedFrom: th.forkedFrom,
      running: false,
      live: false,
    });
  }
  for (const l of live) {
    const tid = l.threadId ?? l.info.threadId ?? null;
    const key = tid ?? `live:${l.info.sessionId}`;
    const prev = out.get(key);
    const ended = l.info.state === "exited" || l.info.state === "failed";
    out.set(key, {
      key,
      threadId: tid,
      sessionId: l.info.sessionId,
      title: l.info.title ?? prev?.title ?? null,
      cwd: l.info.cwd,
      model: l.info.model ?? prev?.model ?? null,
      updatedAt: latest(prev?.updatedAt, l.updatedAt ?? null, l.info.startedAt),
      forkedFrom: prev?.forkedFrom ?? null,
      running: !ended && (l.running || l.info.state === "running" || l.info.state === "waitingApproval"),
      live: !ended,
    });
  }
  return [...out.values()];
}

/** Case-insensitive search over title, folder and id (all words must match). */
export function searchCodexChats(entries: readonly CodexChatEntry[], query: string): CodexChatEntry[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return [...entries];
  return entries.filter((e) => {
    const hay = `${e.title ?? ""} ${e.cwd ?? ""} ${e.threadId ?? ""} ${e.model ?? ""}`.toLowerCase();
    return words.every((w) => hay.includes(w));
  });
}

export interface CodexChatSection {
  group: CodexChatGroup;
  entries: CodexChatEntry[];
}

/** Live sessions first, then saved threads by day; newest first; empty groups dropped. */
export function groupCodexChats(entries: readonly CodexChatEntry[], now: Date): CodexChatSection[] {
  const byGroup = new Map<CodexChatGroup, CodexChatEntry[]>();
  for (const e of entries) {
    const g: CodexChatGroup = e.live ? "live" : chatGroup(e.updatedAt, now);
    const list = byGroup.get(g) ?? [];
    list.push(e);
    byGroup.set(g, list);
  }
  return CODEX_CHAT_GROUPS.flatMap((group) => {
    const list = byGroup.get(group);
    if (!list || list.length === 0) return [];
    list.sort((a, b) => Date.parse(b.updatedAt) - Date.parse(a.updatedAt));
    return [{ group, entries: list }];
  });
}
