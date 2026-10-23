/**
 * Global shortcut dispatch of registry commands (§54). Pure: the window
 * listener in App.tsx feeds it the event and the current context.
 */
import { chordSteps, matchesShortcut } from "./keys";
import type { AppCommand, CommandContext } from "./registry";

type ShortcutEvent = Pick<KeyboardEvent, "key" | "code" | "ctrlKey" | "metaKey" | "shiftKey" | "altKey" | "target">;

/** Whether the event comes from inside an xterm terminal. */
export function isTerminalTarget(target: EventTarget | null): boolean {
  const el = target as { closest?: (sel: string) => unknown } | null;
  return typeof el?.closest === "function" && el.closest(".xterm") != null;
}

/** Whether the event comes from inside a Monaco editor (it owns many Ctrl+Shift chords). */
export function isEditorTarget(target: EventTarget | null): boolean {
  const el = target as { closest?: (sel: string) => unknown } | null;
  return typeof el?.closest === "function" && el.closest(".monaco-editor") != null;
}

/**
 * The registry command a keydown should run, or null. While focus is in a
 * terminal only commands marked `inTerminal` are taken; every other key goes
 * to the shell (e.g. Ctrl+P = previous history).
 */
export function shortcutCommand(e: ShortcutEvent, list: readonly AppCommand[], ctx: CommandContext): AppCommand | null {
  const inTerminal = isTerminalTarget(e.target);
  const inEditor = isEditorTarget(e.target);
  for (const cmd of list) {
    if (!cmd.shortcut || !matchesShortcut(e, cmd.shortcut)) continue;
    if (inTerminal && !cmd.inTerminal) continue;
    if (inEditor && cmd.inEditor === false) continue;
    if (cmd.enabled && !cmd.enabled(ctx)) continue;
    return cmd;
  }
  return null;
}

/** How long the second key of a chord (`Ctrl+K Ctrl+O`) is awaited after the first. */
export const CHORD_TIMEOUT_MS = 1500;

/** Keys that only change modifiers (they never end a pending chord). */
export function isModifierKey(key: string): boolean {
  return key === "Control" || key === "Shift" || key === "Alt" || key === "Meta" || key === "AltGraph" || key === "OS";
}

/**
 * The registry command whose chord is `prefix` followed by this event
 * (`Ctrl+K` then `Ctrl+O` → the `Ctrl+K Ctrl+O` command), or null. Same
 * terminal and `enabled` rules as `shortcutCommand`.
 */
export function chordCommand(
  prefix: string,
  e: ShortcutEvent,
  list: readonly AppCommand[],
  ctx: CommandContext,
): AppCommand | null {
  const inTerminal = isTerminalTarget(e.target);
  for (const cmd of list) {
    if (!cmd.shortcut) continue;
    const steps = chordSteps(cmd.shortcut);
    const [first, second] = steps;
    if (steps.length !== 2 || !first || !second) continue;
    if (first.toLowerCase() !== prefix.toLowerCase() || !matchesShortcut(e, second)) continue;
    if (inTerminal && !cmd.inTerminal) continue;
    if (cmd.enabled && !cmd.enabled(ctx)) continue;
    return cmd;
  }
  return null;
}
