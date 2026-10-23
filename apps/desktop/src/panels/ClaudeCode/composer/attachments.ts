/**
 * Pure helpers of the composer: attachments (picked `@file` mentions and the
 * attached editor selection), slash-command suggestions and the final text
 * sent to the CLI.
 */
import type { ClaudeSlashCommand } from "@orkestr/protocol";

import type { EditorSelection } from "../../../features/claudeCode/bridge";
import { selectionPrompt } from "../../../features/claudeCode/model";
import type { ComposerSendPayload } from "../types";

/** Something attached to a message. */
export type ComposerAttachment =
  | { kind: "file"; path: string }
  | ({ kind: "selection" } & EditorSelection);

/** What the composer sends: the contract payload plus its attachments. */
export interface ComposerSendPayloadWithAttachments extends ComposerSendPayload {
  attachments: ComposerAttachment[];
}

/** Built-in Claude Code commands always offered (the CLI may report more). */
export const BUILTIN_SLASH_COMMANDS = ["compact", "clear", "init", "cost", "review", "model"] as const;
export type BuiltinSlashCommand = (typeof BUILTIN_SLASH_COMMANDS)[number];

export function isBuiltinSlashCommand(name: string): name is BuiltinSlashCommand {
  return (BUILTIN_SLASH_COMMANDS as readonly string[]).includes(name);
}

/** A slash suggestion; `description: null` for built-ins (localized by the view). */
export interface SlashSuggestion {
  name: string;
  description: string | null;
  argumentHint: string | null;
}

/** Built-ins first, then the CLI's commands (deduplicated), filtered by prefix. */
export function slashSuggestions(commands: readonly ClaudeSlashCommand[], query: string, limit = 8, builtins = true): SlashSuggestion[] {
  const q = query.toLowerCase();
  const seen = new Set<string>();
  const out: SlashSuggestion[] = [];
  const push = (s: SlashSuggestion) => {
    if (seen.has(s.name) || !s.name.toLowerCase().startsWith(q)) return;
    seen.add(s.name);
    out.push(s);
  };
  for (const name of builtins ? BUILTIN_SLASH_COMMANDS : []) {
    const cli = commands.find((c) => c.name === name);
    push({ name, description: null, argumentHint: cli?.argumentHint ?? null });
  }
  for (const c of commands) push({ name: c.name, description: c.description || null, argumentHint: c.argumentHint });
  return out.slice(0, limit);
}

/** The `@` token `insertMention` writes for a path. */
export function mentionToken(path: string): string {
  return `@${path.includes(" ") ? `"${path}"` : path}`;
}

/** Picked files whose mention is still in the text (in pick order, unique). */
export function mentionedFiles(text: string, picked: readonly string[]): string[] {
  const out: string[] = [];
  for (const path of picked) {
    if (out.includes(path)) continue;
    const token = mentionToken(path);
    let idx = text.indexOf(token);
    while (idx !== -1) {
      const before = idx === 0 ? " " : text[idx - 1] ?? " ";
      const after = text[idx + token.length] ?? " ";
      if (/\s/.test(before) && /\s/.test(after)) {
        out.push(path);
        break;
      }
      idx = text.indexOf(token, idx + 1);
    }
  }
  return out;
}

/** `12` or `12-20`. */
export function lineRange(sel: Pick<EditorSelection, "startLine" | "endLine">): string {
  return sel.startLine === sel.endLine ? `${sel.startLine}` : `${sel.startLine}-${sel.endLine}`;
}

/** Builds the payload: the selection (if any) goes before the typed text as `@path:lines` + a fenced block. */
export function buildPayload(
  body: string,
  picked: readonly string[],
  selection: EditorSelection | null,
): ComposerSendPayloadWithAttachments {
  const attachments: ComposerAttachment[] = [];
  if (selection) attachments.push({ kind: "selection", ...selection });
  for (const path of mentionedFiles(body, picked)) attachments.push({ kind: "file", path });
  const text = selection ? `${selectionPrompt(selection)}${body}` : body;
  return { text, attachments };
}
