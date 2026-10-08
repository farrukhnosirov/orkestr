/**
 * Helpers of the Claude Code panel's "hidden features" (pure; unit-tested):
 * plan usage tones, per-chat totals, last answer / last prompt, Markdown
 * export and image attachments.
 */
import type { ClaudeLimitWindow } from "@orkestr/protocol";

import { inputTokens, type ChatItem, type ChatState } from "./model";

export type UsageTone = "ok" | "warn" | "danger";

/** Color tone of a limit bar. */
export function usageTone(w: Pick<ClaudeLimitWindow, "percent" | "severity"> | null | undefined): UsageTone {
  if (!w) return "ok";
  const sev = w.severity?.toLowerCase() ?? "";
  if (w.percent >= 90 || sev === "critical" || sev === "exceeded" || sev === "error") return "danger";
  if (w.percent >= 70 || sev === "warning") return "warn";
  return "ok";
}

/** Milliseconds until `resetsAt` (≥ 0), or null. */
export function msUntil(resetsAt: string | null | undefined, now: number): number | null {
  if (!resetsAt) return null;
  const t = Date.parse(resetsAt);
  return Number.isFinite(t) ? Math.max(0, t - now) : null;
}

export interface ChatTotals {
  turns: number;
  inputTokens: number;
  outputTokens: number;
  costUsd: number;
  durationMs: number;
}

function flat(items: readonly ChatItem[]): ChatItem[] {
  return items.flatMap((it) => (it.kind === "tool" ? [it, ...flat(it.children)] : [it]));
}

/** Sum of every finished turn of the chat. */
export function chatTotals(chat: ChatState): ChatTotals {
  const t: ChatTotals = { turns: 0, inputTokens: 0, outputTokens: 0, costUsd: 0, durationMs: 0 };
  for (const it of chat.items) {
    if (it.kind !== "result") continue;
    t.turns += 1;
    t.inputTokens += inputTokens(it.usage);
    t.outputTokens += it.usage.outputTokens;
    t.costUsd += it.costUsd ?? 0;
    t.durationMs += it.durationMs ?? 0;
  }
  return t;
}

/** Text of Claude's last top-level answer. */
export function lastAnswer(chat: ChatState): string | null {
  for (let i = chat.items.length - 1; i >= 0; i--) {
    const it = chat.items[i];
    if (it?.kind === "assistant" && !it.synthetic && it.text.trim()) return it.text;
  }
  return null;
}

/** The last message the user sent (slash commands excluded). */
export function lastPrompt(chat: ChatState): string | null {
  for (let i = chat.items.length - 1; i >= 0; i--) {
    const it = chat.items[i];
    if (it?.kind === "user" && !it.text.trim().startsWith("/")) return it.text;
  }
  return null;
}

export interface ExportLabels {
  user: string;
  claude: string;
  tool: string;
}

/** The conversation as Markdown (user / Claude messages and tool calls). */
export function chatMarkdown(chat: ChatState, title: string, labels: ExportLabels): string {
  const out: string[] = [`# ${title}`, ""];
  for (const it of flat(chat.items)) {
    if (it.kind === "user") out.push(`## ${labels.user}`, "", it.text, "");
    else if (it.kind === "assistant" && it.text.trim()) out.push(`## ${labels.claude}`, "", it.text, "");
    else if (it.kind === "tool") out.push(`> ${labels.tool}: \`${it.name}\``, "");
  }
  return `${out.join("\n").trimEnd()}\n`;
}

/** A file name for an exported chat. */
export function exportFileName(title: string, date: Date): string {
  const slug = title
    .toLowerCase()
    .replace(/[^\p{L}\p{N}]+/gu, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 48);
  const d = date.toISOString().slice(0, 10);
  return `claude-${slug || "chat"}-${d}.md`;
}

export const IMAGE_TYPES = ["image/png", "image/jpeg", "image/gif", "image/webp"] as const;
/** Largest image accepted (bytes). */
export const MAX_IMAGE_BYTES = 5 * 1024 * 1024;
/** Most images per message. */
export const MAX_IMAGES = 10;

export interface ImageAttachment {
  id: string;
  name: string;
  mediaType: string;
  /** Base64 without the `data:` prefix. */
  data: string;
}

/** Splits a `data:` URL into media type + base64 (null when not a supported image). */
export function parseDataUrl(url: string): { mediaType: string; data: string } | null {
  const m = /^data:([^;,]+);base64,(.*)$/s.exec(url);
  if (!m || !m[1] || !m[2]) return null;
  if (!(IMAGE_TYPES as readonly string[]).includes(m[1])) return null;
  return { mediaType: m[1], data: m[2] };
}
