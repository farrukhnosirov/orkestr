/**
 * Command registry (§54). Features append commands here; the command palette,
 * keyboard shortcuts and voice commands (Phase 3) all read from it.
 */
import type { LucideIcon } from "lucide-react";

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
  /** Display-only shortcut, e.g. `Ctrl+Shift+T`. */
  shortcut?: string;
  /** Whether the command can run now. */
  enabled?: (ctx: CommandContext) => boolean;
  run: (ctx: CommandContext) => void | Promise<void>;
}

export const commands: AppCommand[] = [];
