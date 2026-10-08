/**
 * Command catalogue of the «Команды» drawer (pure; unit-tested).
 *
 * Only commands the CLI reported (its `initialize` answer /
 * `commands_changed`) are shown. Built-ins Orkestr knows get a curated
 * group, icon and localized one-line description (`claudeCodePanel:cmd.<name>`);
 * custom commands, skills, plugin and MCP prompts keep the CLI's own
 * description and land in «Свои команды и навыки».
 */
import type { ClaudeSlashCommand } from "@orkestr/protocol";

export type CommandGroup = "context" | "code" | "settings" | "account" | "diagnostics" | "custom";

export const COMMAND_GROUPS: readonly CommandGroup[] = ["context", "code", "settings", "account", "diagnostics", "custom"];

/** Icon keys (mapped to lucide icons by the panel). */
export type CommandIcon =
  | "archive"
  | "eraser"
  | "fileText"
  | "search"
  | "shield"
  | "coins"
  | "gauge"
  | "layers"
  | "brain"
  | "bot"
  | "plug"
  | "anchor"
  | "lock"
  | "cpu"
  | "settings"
  | "info"
  | "stethoscope"
  | "download"
  | "undo"
  | "folderPlus"
  | "palette"
  | "listTodo"
  | "bug"
  | "newspaper"
  | "terminal"
  | "keyboard"
  | "history"
  | "logIn"
  | "logOut"
  | "zap"
  | "sparkles"
  | "repeat"
  | "pencil"
  | "puzzle"
  | "slash";

/** What the button does besides sending `/<name>` to the CLI. */
export type NativeAction = "usage" | "context" | "mcp" | "export" | "model" | "addDir" | "rename" | "login";

export interface CuratedCommand {
  group: Exclude<CommandGroup, "custom">;
  icon: CommandIcon;
  /** Shows an argument field (optional argument). */
  arg?: boolean;
  /** Handled by Orkestr's own UI instead of the CLI. */
  native?: NativeAction;
  /** Ends / restarts the conversation (asks for confirmation). */
  destructive?: boolean;
}

/** Built-ins with curated descriptions (keys of `claudeCodePanel:cmd`). */
export const CURATED: Readonly<Record<string, CuratedCommand>> = {
  compact: { group: "context", icon: "archive", arg: true },
  clear: { group: "context", icon: "eraser", destructive: true },
  context: { group: "context", icon: "layers", native: "context" },
  memory: { group: "context", icon: "brain" },
  "add-dir": { group: "context", icon: "folderPlus", native: "addDir" },
  rewind: { group: "context", icon: "undo" },
  export: { group: "context", icon: "download", native: "export" },
  resume: { group: "context", icon: "history" },
  recap: { group: "context", icon: "fileText" },
  rename: { group: "context", icon: "pencil", native: "rename" },
  todos: { group: "context", icon: "listTodo" },
  init: { group: "code", icon: "fileText" },
  review: { group: "code", icon: "search", arg: true },
  "code-review": { group: "code", icon: "search", arg: true },
  "security-review": { group: "code", icon: "shield" },
  ultrareview: { group: "code", icon: "search", arg: true },
  simplify: { group: "code", icon: "sparkles" },
  verify: { group: "code", icon: "shield" },
  loop: { group: "code", icon: "repeat", arg: true },
  model: { group: "settings", icon: "cpu", native: "model" },
  effort: { group: "settings", icon: "gauge", arg: true },
  fast: { group: "settings", icon: "zap" },
  config: { group: "settings", icon: "settings" },
  agents: { group: "settings", icon: "bot" },
  mcp: { group: "settings", icon: "plug", native: "mcp" },
  hooks: { group: "settings", icon: "anchor" },
  permissions: { group: "settings", icon: "lock" },
  "output-style": { group: "settings", icon: "palette", arg: true },
  autocompact: { group: "settings", icon: "archive", arg: true },
  vim: { group: "settings", icon: "keyboard" },
  "terminal-setup": { group: "settings", icon: "terminal" },
  "reload-plugins": { group: "settings", icon: "puzzle" },
  "reload-skills": { group: "settings", icon: "puzzle" },
  usage: { group: "account", icon: "gauge", native: "usage" },
  cost: { group: "account", icon: "coins" },
  "extra-usage": { group: "account", icon: "coins" },
  "usage-credits": { group: "account", icon: "coins" },
  login: { group: "account", icon: "logIn", native: "login" },
  logout: { group: "account", icon: "logOut", destructive: true },
  status: { group: "diagnostics", icon: "info" },
  doctor: { group: "diagnostics", icon: "stethoscope" },
  debug: { group: "diagnostics", icon: "bug", arg: true },
  bug: { group: "diagnostics", icon: "bug", arg: true },
  insights: { group: "diagnostics", icon: "info" },
  "release-notes": { group: "diagnostics", icon: "newspaper" },
};

export interface CatalogEntry {
  name: string;
  group: CommandGroup;
  icon: CommandIcon;
  /** Has a curated localized description (`claudeCodePanel:cmd.<name>`). */
  curated: boolean;
  /** The CLI's own description (English). */
  cliDescription: string;
  argumentHint: string | null;
  arg: boolean;
  native: NativeAction | null;
  destructive: boolean;
}

export interface CatalogSection {
  group: CommandGroup;
  entries: CatalogEntry[];
}

/** Catalogue entries for the commands the CLI reported (deduplicated). */
export function catalogEntries(commands: readonly ClaudeSlashCommand[]): CatalogEntry[] {
  const seen = new Set<string>();
  const out: CatalogEntry[] = [];
  for (const c of commands) {
    const name = c.name.trim();
    if (!name || seen.has(name) || name.startsWith("__")) continue;
    seen.add(name);
    const cur = CURATED[name];
    const hint = c.argumentHint?.trim() ? c.argumentHint.trim() : null;
    out.push({
      name,
      group: cur?.group ?? "custom",
      icon: cur?.icon ?? (c.builtin === false ? "puzzle" : "slash"),
      curated: cur !== undefined,
      cliDescription: c.description,
      argumentHint: hint,
      arg: cur?.arg ?? hint !== null,
      native: cur?.native ?? null,
      destructive: cur?.destructive ?? false,
    });
  }
  return out;
}

/**
 * Filters by a query (name, CLI description and the localized description via
 * `describe`) and groups in the fixed order; names sorted inside a group.
 */
export function catalogSections(
  entries: readonly CatalogEntry[],
  query: string,
  describe: (e: CatalogEntry) => string,
  group: CommandGroup | "all" = "all",
): CatalogSection[] {
  const words = query.toLowerCase().replace(/^\//, "").split(/\s+/).filter(Boolean);
  const match = (e: CatalogEntry) => {
    if (group !== "all" && e.group !== group) return false;
    if (words.length === 0) return true;
    const hay = `${e.name} ${e.cliDescription} ${describe(e)}`.toLowerCase();
    return words.every((w) => hay.includes(w));
  };
  return COMMAND_GROUPS.flatMap((g) => {
    const list = entries.filter((e) => e.group === g && match(e)).sort((a, b) => a.name.localeCompare(b.name));
    return list.length > 0 ? [{ group: g, entries: list }] : [];
  });
}

/** The text sent to the CLI for a command with an optional argument. */
export function commandText(name: string, arg: string): string {
  const a = arg.replace(/\s+/g, " ").trim();
  return a ? `/${name} ${a}` : `/${name}`;
}
