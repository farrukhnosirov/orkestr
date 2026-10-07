/**
 * Pure helpers of the Terminal panel (unit-tested; no DOM / xterm imports).
 */
import type { ShellDescriptor, TerminalSession } from "@orkestr/protocol";

import { shortcutKey } from "../../features/commands/keys";
import { t } from "../../i18n";
import { describeError } from "../../i18n/errors";

/** What the terminal does with a keydown before xterm sees it. */
export type TerminalKeyAction = "copy" | "paste" | "global" | null;

/**
 * Clipboard / pass-through keys of the terminal, layout-independent (on
 * ЙЦУКЕН Ctrl+C arrives as `key: "с"`, Ctrl+V as `"м"`): Ctrl+Shift+C/V and
 * Ctrl/Shift+Insert always; plain Ctrl+C copies only with a selection —
 * otherwise it is left to xterm, which sends ^C (SIGINT) from the key code.
 * `global`: Ctrl+` is left to the app's "Open Terminal" shortcut.
 */
export function terminalKeyAction(
  e: Pick<KeyboardEvent, "key" | "code" | "ctrlKey" | "metaKey" | "shiftKey">,
  hasSelection: boolean,
): TerminalKeyAction {
  const ctrl = e.ctrlKey || e.metaKey;
  const key = shortcutKey(e);
  if (ctrl && (key === "`" || e.code === "Backquote")) return "global";
  if (ctrl && (key === "c" || key === "insert") && (e.shiftKey || hasSelection)) return "copy";
  if ((ctrl && key === "v") || (e.shiftKey && key === "insert")) return "paste";
  return null;
}

const encoder = new TextEncoder();
const decoder = new TextDecoder();

/** UTF-8 byte length of a string (offsets on the wire are byte offsets). */
export function utf8Length(s: string): number {
  return encoder.encode(s).length;
}

/**
 * De-duplicates live output against what was already written.
 *
 * `written` is the absolute byte offset up to which output is on screen (the
 * history's `endOffset` after a replay). Returns the part of `data` (starting
 * at absolute byte `offset`) that is new, plus the new `written` offset.
 * Chunks without an offset (older cores) are always written.
 */
export function dedupeChunk(
  written: number,
  offset: number | undefined,
  data: string,
): { text: string; written: number } {
  if (offset === undefined) return { text: data, written: written + utf8Length(data) };
  const bytes = encoder.encode(data);
  const end = offset + bytes.length;
  if (end <= written) return { text: "", written };
  if (offset >= written) return { text: data, written: end };
  // Overlap: drop the already-written prefix. Offsets always fall on code point
  // boundaries server-side, so the slice decodes cleanly.
  return { text: decoder.decode(bytes.subarray(written - offset)), written: end };
}

/** Reads a CSS custom property from the document (falls back when absent). */
function cssVar(name: string, fallback: string, root?: HTMLElement): string {
  if (typeof document === "undefined") return fallback;
  const el = root ?? document.documentElement;
  const v = getComputedStyle(el).getPropertyValue(name).trim();
  return v || fallback;
}

/** xterm.js theme (subset of `ITheme`) derived from the active @orkestr/ui tokens. */
export interface TerminalTheme {
  background: string;
  foreground: string;
  cursor: string;
  cursorAccent: string;
  selectionBackground: string;
  black: string;
  red: string;
  green: string;
  yellow: string;
  blue: string;
  magenta: string;
  cyan: string;
  white: string;
  brightBlack: string;
  brightRed: string;
  brightGreen: string;
  brightYellow: string;
  brightBlue: string;
  brightMagenta: string;
  brightCyan: string;
  brightWhite: string;
}

/** Builds the xterm theme from the current design tokens. */
export function themeFromTokens(root?: HTMLElement): TerminalTheme {
  const v = (name: string, fallback: string) => cssVar(name, fallback, root);
  const text = v("--ok-color-text", "#e8ecff");
  const muted = v("--ok-color-text-muted", "#9aa3c8");
  const subtle = v("--ok-color-text-subtle", "#646d96");
  const accent = v("--ok-color-accent", "#22d3ee");
  const violet = v("--ok-color-violet", "#a78bfa");
  const magenta = v("--ok-color-magenta", "#f472b6");
  const success = v("--ok-color-success", "#34d399");
  const warning = v("--ok-color-warning", "#fbbf24");
  const danger = v("--ok-color-danger", "#fb7185");
  const info = v("--ok-color-info", "#60a5fa");
  return {
    // The panel's glass surface shows through (xterm `allowTransparency`).
    background: "rgba(0, 0, 0, 0)",
    foreground: text,
    cursor: accent,
    cursorAccent: v("--ok-color-bg", "#04050d"),
    selectionBackground: v("--ok-color-surface-active", "rgba(80, 110, 210, 0.35)"),
    black: v("--ok-color-bg-elevated", "#0a0d1c"),
    red: danger,
    green: success,
    yellow: warning,
    blue: info,
    magenta: violet,
    cyan: accent,
    white: muted,
    brightBlack: subtle,
    brightRed: danger,
    brightGreen: success,
    brightYellow: warning,
    brightBlue: info,
    brightMagenta: magenta,
    brightCyan: accent,
    brightWhite: text,
  };
}

/** Monospace font stack: JetBrains Mono → Cascadia → system monospace. */
export function terminalFontFamily(root?: HTMLElement): string {
  return cssVar("--ok-font-mono", '"JetBrains Mono", "Cascadia Code", "Cascadia Mono", Consolas, monospace', root);
}

/** Whether the user may type into the session. */
export function isReadOnly(s: TerminalSession): boolean {
  return s.readOnly || s.kind === "commandView" || s.owner.kind === "agent";
}

/** Whether the session belongs to an agent. */
export function isAgentSession(s: TerminalSession): boolean {
  return s.owner.kind === "agent";
}

/** Sessions shown for a project: its own plus project-less user terminals. */
export function sessionsForProject(all: readonly TerminalSession[], projectId: string | null): TerminalSession[] {
  return all.filter((s) => s.projectId === projectId || s.projectId === null);
}

/** Short label for the tab tooltip / status line. */
export function sessionSubtitle(s: TerminalSession): string {
  if (s.status === "exited") {
    return s.exitCode === null ? t("terminal:subtitle.exited") : t("terminal:subtitle.exitedCode", { code: s.exitCode });
  }
  if (s.kind === "commandView") return t("terminal:subtitle.commandView");
  return s.pid === null ? s.shellId : t("terminal:subtitle.pid", { shell: s.shellId, pid: s.pid });
}

/** Default shell first, then by name. */
export function orderShells(shells: readonly ShellDescriptor[]): ShellDescriptor[] {
  return [...shells].sort((a, b) => Number(b.isDefault) - Number(a.isDefault) || a.name.localeCompare(b.name));
}

/** Validates a tab title the same way the core does (1..=80 printable chars). */
export function normalizeTitle(input: string): string | null {
  const t = input.trim();
  if (t.length === 0 || [...t].length > 80 || /\p{Cc}/u.test(t)) return null;
  return t;
}

/** User-facing message for an RPC/client error (localized code label + core detail). */
export function errorMessage(e: unknown): string {
  return describeError(e);
}

/** Whether an error means "the session is already gone" (closing it then is fine). */
export function isNotFound(e: unknown): boolean {
  if (e && typeof e === "object" && "code" in e && (e as { code: unknown }).code === "not_found") return true;
  const raw = e && typeof e === "object" && "message" in e ? (e as { message: unknown }).message : null;
  return typeof raw === "string" && /not found/i.test(raw);
}

/** Next tab to activate after closing `closingId`. */
export function nextActiveAfterClose(ids: readonly string[], closingId: string): string | null {
  const idx = ids.indexOf(closingId);
  if (idx < 0) return ids[0] ?? null;
  const rest = ids.filter((id) => id !== closingId);
  return rest[Math.min(idx, rest.length - 1)] ?? null;
}
