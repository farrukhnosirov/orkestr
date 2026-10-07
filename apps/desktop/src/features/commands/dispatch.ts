/**
 * Global shortcut dispatch of registry commands (§54). Pure: the window
 * listener in App.tsx feeds it the event and the current context.
 */
import { matchesShortcut } from "./keys";
import type { AppCommand, CommandContext } from "./registry";

type ShortcutEvent = Pick<KeyboardEvent, "key" | "code" | "ctrlKey" | "metaKey" | "shiftKey" | "altKey" | "target">;

/** Whether the event comes from inside an xterm terminal. */
export function isTerminalTarget(target: EventTarget | null): boolean {
  const el = target as { closest?: (sel: string) => unknown } | null;
  return typeof el?.closest === "function" && el.closest(".xterm") != null;
}

/**
 * The registry command a keydown should run, or null. While focus is in a
 * terminal only commands marked `inTerminal` are taken; every other key goes
 * to the shell (e.g. Ctrl+P = previous history).
 */
export function shortcutCommand(e: ShortcutEvent, list: readonly AppCommand[], ctx: CommandContext): AppCommand | null {
  const inTerminal = isTerminalTarget(e.target);
  for (const cmd of list) {
    if (!cmd.shortcut || !matchesShortcut(e, cmd.shortcut)) continue;
    if (inTerminal && !cmd.inTerminal) continue;
    if (cmd.enabled && !cmd.enabled(ctx)) continue;
    return cmd;
  }
  return null;
}
