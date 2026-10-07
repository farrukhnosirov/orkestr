/**
 * Pure helpers of the Claude Code tool cards: reading tool inputs, parsing
 * the CLI's textual tool results (Read / Glob / Grep / LS / WebSearch) and a
 * lightweight shell-risk heuristic. No React, no i18n — unit-testable.
 */
import type { JsonValue } from "@orkestr/protocol";

import { field } from "../../../features/claudeCode/model";

/** Card families. */
export type ToolKind =
  | "read"
  | "write"
  | "edit"
  | "multiEdit"
  | "notebookEdit"
  | "bash"
  | "bashOutput"
  | "killShell"
  | "glob"
  | "grep"
  | "ls"
  | "todo"
  | "webFetch"
  | "webSearch"
  | "task"
  | "plan"
  | "mcp"
  | "unknown";

export function toolKind(name: string): ToolKind {
  switch (name) {
    case "Read":
      return "read";
    case "Write":
      return "write";
    case "Edit":
      return "edit";
    case "MultiEdit":
      return "multiEdit";
    case "NotebookEdit":
      return "notebookEdit";
    case "Bash":
    case "PowerShell":
      return "bash";
    case "BashOutput":
      return "bashOutput";
    case "KillShell":
    case "KillBash":
      return "killShell";
    case "Glob":
      return "glob";
    case "Grep":
      return "grep";
    case "LS":
      return "ls";
    case "TodoWrite":
      return "todo";
    case "WebFetch":
      return "webFetch";
    case "WebSearch":
      return "webSearch";
    case "Task":
    case "Agent":
      return "task";
    case "ExitPlanMode":
      return "plan";
    default:
      return name.startsWith("mcp__") ? "mcp" : "unknown";
  }
}

/** String field of an object input (null when absent / not a string). */
export function strField(input: JsonValue, key: string): string | null {
  const v = field(input, key);
  return typeof v === "string" ? v : null;
}

/** Number field of an object input. */
export function numField(input: JsonValue, key: string): number | null {
  const v = field(input, key);
  return typeof v === "number" && Number.isFinite(v) ? v : null;
}

/** Boolean field of an object input. */
export function boolField(input: JsonValue, key: string): boolean {
  return field(input, key) === true;
}

/** `mcp__server__tool` → `{ server, tool }`. */
export function mcpParts(name: string): { server: string; tool: string } {
  const rest = name.slice("mcp__".length);
  const i = rest.indexOf("__");
  if (i < 0) return { server: rest, tool: "" };
  return { server: rest.slice(0, i), tool: rest.slice(i + 2) };
}

/** Approximate added / removed line counts of an edit (common prefix / suffix lines trimmed; O(n)). */
export function diffStats(before: string, after: string): { added: number; removed: number } {
  const a = before === "" ? [] : before.split("\n");
  const b = after === "" ? [] : after.split("\n");
  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start++;
  let endA = a.length;
  let endB = b.length;
  while (endA > start && endB > start && a[endA - 1] === b[endB - 1]) {
    endA--;
    endB--;
  }
  return { added: endB - start, removed: endA - start };
}

/** One numbered line of a `Read` result. */
export interface NumberedLine {
  line: number;
  text: string;
}

/** Parses the CLI's `cat -n`-style Read output (`   12→text` or `   12\ttext`). */
export function parseReadOutput(content: string): NumberedLine[] {
  const out: NumberedLine[] = [];
  for (const raw of content.split("\n")) {
    const m = /^\s*(\d+)(?:→|\t)(.*)$/.exec(raw);
    if (m && m[1] !== undefined) out.push({ line: Number(m[1]), text: m[2] ?? "" });
  }
  return out;
}

/** One entry of a Glob / Grep result list. */
export interface ResultEntry {
  path: string;
  /** Line number (Grep content mode with `-n`). */
  line?: number;
  /** Matched text (Grep content mode) or a match count (count mode). */
  text?: string;
  count?: number;
}

const SUMMARY_LINE = /^(Found \d+ (files?|matches?|lines?)|No (files|matches) found|\(Results are truncated.*\)|Found \d+ total occurrences.*)$/i;

/** Whether a result line is a CLI summary (not an entry). */
export function isSummaryLine(line: string): boolean {
  return SUMMARY_LINE.test(line.trim());
}

/** Parses a Glob result (one path per line). Summary lines are dropped. */
export function parsePathList(content: string): ResultEntry[] {
  return content
    .split("\n")
    .map((l) => l.trim())
    .filter((l) => l !== "" && !isSummaryLine(l))
    .map((path) => ({ path }));
}

/** Parses a Grep result in any output mode (files, `path:line:text`, `path:count`). */
export function parseGrepOutput(content: string): ResultEntry[] {
  const out: ResultEntry[] = [];
  for (const raw of content.split("\n")) {
    const l = raw.replace(/\r$/, "");
    if (l.trim() === "" || isSummaryLine(l) || l === "--") continue;
    const withLine = /^((?:[a-zA-Z]:)?[^:]+):(\d+)[:-](.*)$/.exec(l);
    if (withLine && withLine[1] && withLine[2]) {
      out.push({ path: withLine[1], line: Number(withLine[2]), text: withLine[3] ?? "" });
      continue;
    }
    const count = /^((?:[a-zA-Z]:)?[^:]+):(\d+)$/.exec(l);
    if (count && count[1] && count[2]) {
      out.push({ path: count[1], count: Number(count[2]) });
      continue;
    }
    out.push({ path: l.trim() });
  }
  return out;
}

/** One entry of an LS tree. */
export interface LsEntry {
  /** Full path (joined from the tree). */
  path: string;
  name: string;
  depth: number;
  dir: boolean;
}

/** Parses the CLI's LS tree (`- root/` then indented `- name`). The root line is depth 0. */
export function parseLsOutput(content: string): LsEntry[] {
  const out: LsEntry[] = [];
  const stack: string[] = [];
  for (const raw of content.split("\n")) {
    const m = /^(\s*)- (.+?)\s*$/.exec(raw);
    if (!m || m[2] === undefined) continue;
    const depth = Math.floor((m[1] ?? "").length / 2);
    const name = m[2];
    const dir = /[\\/]$/.test(name);
    const clean = name.replace(/[\\/]+$/, "");
    stack.length = depth;
    const parent = depth > 0 ? (stack[depth - 1] ?? "") : "";
    const path = parent ? `${parent}/${clean}` : clean;
    stack[depth] = path;
    out.push({ path, name: clean, depth, dir });
  }
  return out;
}

/** A link of a WebSearch result. */
export interface SearchLink {
  title: string;
  url: string;
}

/** Splits a WebSearch result into its `Links: [...]` JSON lists and the remaining text. */
export function parseWebSearch(content: string): { links: SearchLink[]; text: string } {
  const links: SearchLink[] = [];
  const text = content.replace(/^Links:\s*(\[.*\])\s*$/gm, (whole, json: string) => {
    try {
      const parsed: unknown = JSON.parse(json);
      if (!Array.isArray(parsed)) return whole;
      for (const l of parsed) {
        if (l && typeof l === "object" && typeof (l as SearchLink).url === "string") {
          const { url, title } = l as SearchLink;
          links.push({ url, title: typeof title === "string" && title ? title : url });
        }
      }
      return "";
    } catch {
      return whole;
    }
  });
  return { links, text: text.replace(/^Web search results for query:.*$/m, "").trim() };
}

/** Patterns of the local (UI-only) shell risk heuristic. The core guard is authoritative. */
const RISKY: readonly RegExp[] = [
  /\brm\s+(-[a-zA-Z]*[rf][a-zA-Z]*\s+)+/,
  /\bRemove-Item\b[^|;]*-Recurse/i,
  /\b(del|rd|rmdir)\s+\/[sq]/i,
  /\bgit\s+(push\s+[^|;]*(--force|-f\b)|reset\s+--hard|clean\s+-[a-zA-Z]*f|checkout\s+--\s|branch\s+-D)/,
  /\b(curl|wget|iwr|Invoke-WebRequest)\b[^|]*\|\s*(sh|bash|zsh|iex|Invoke-Expression|python)\b/i,
  /\b(sudo|doas|runas)\b/,
  /\bchmod\s+(-R\s+)?777\b/,
  /\b(mkfs|dd\s+if=|format\s+[a-zA-Z]:|diskpart|shutdown|reboot)\b/i,
  /\b(DROP\s+(TABLE|DATABASE)|TRUNCATE\s+TABLE)\b/i,
  /\bnpm\s+publish\b|\bcargo\s+publish\b/,
  /\b(kubectl\s+delete|terraform\s+(apply|destroy)|docker\s+(system\s+prune|rm\s+-f))\b/,
  /(^|[\s;&|])>\s*\/dev\/sd[a-z]/,
];

/** Lightweight local estimate of a shell command's risk (true = looks risky). */
export function looksRisky(command: string): boolean {
  return RISKY.some((r) => r.test(command));
}

/** Line count of a text (0 for empty). */
export function lineCount(text: string): number {
  return text === "" ? 0 : text.split("\n").length;
}
