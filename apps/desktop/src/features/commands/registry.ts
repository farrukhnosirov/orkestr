/**
 * Command registry (§54). Features append commands here; the command palette,
 * keyboard shortcuts and voice commands (Phase 3) all read from it.
 */
import type { LucideIcon } from "lucide-react";

import { providerCommands } from "../providers";
import { taskCommands } from "../../panels/Tasks/commands";
import { GIT_COMMANDS } from "../../panels/Git/commands";
import { terminalCommands } from "../../panels/Terminal/commands";
import { FILE_COMMANDS } from "../files/commands";

export interface CommandContext {
  projectId: string | null;
}

export interface AppCommand {
  /** Stable id, e.g. `project.create`. */
  id: string;
  title: string;
  /** Grouping in the palette. */
  group: string;
  icon?: LucideIcon;
  /** Extra search terms (any language). */
  keywords?: string[];
  /** Shortcut, e.g. `Ctrl+Shift+T` (dispatched globally via `matchesShortcut`). */
  shortcut?: string;
  /** Whether the command can run now. */
  enabled?: (ctx: CommandContext) => boolean;
  run: (ctx: CommandContext) => void | Promise<void>;
}

export const commands: AppCommand[] = [];
commands.push(...providerCommands);
commands.push(...taskCommands);

// Git / snapshots (Phase 1, vcs-snapshots)
commands.push(...GIT_COMMANDS);
commands.push(...terminalCommands);

/**
 * Whether a keyboard event matches a shortcut string like `Ctrl+Shift+T` or
 * `Ctrl+\``. `Ctrl` also matches ⌘ on macOS. Modifiers must match exactly.
 */
export function matchesShortcut(e: Pick<KeyboardEvent, "key" | "code" | "ctrlKey" | "metaKey" | "shiftKey" | "altKey">, shortcut: string): boolean {
  const parts = shortcut.split("+").map((p) => p.trim().toLowerCase());
  const key = parts.pop();
  if (!key) return false;
  const want = { ctrl: parts.includes("ctrl"), shift: parts.includes("shift"), alt: parts.includes("alt") };
  if (want.ctrl !== (e.ctrlKey || e.metaKey) || want.shift !== e.shiftKey || want.alt !== e.altKey) return false;
  if (key === "`") return e.key === "`" || e.code === "Backquote";
  return e.key.toLowerCase() === key;
}

// files-editor (Phase 1)
commands.push(...FILE_COMMANDS);
