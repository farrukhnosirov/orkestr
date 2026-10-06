/**
 * Push-to-talk hold hotkey (default `Ctrl+Space`, §33): parsing, matching a
 * keydown, and recognizing the release (either the main key or one of the
 * modifiers going up ends the hold). Pure, unit-tested.
 */

export interface HoldHotkey {
  ctrl: boolean;
  shift: boolean;
  alt: boolean;
  /** `KeyboardEvent.code` of the main key, e.g. `Space`, `KeyV`, `F9`. */
  code: string;
}

const NAMED: Record<string, string> = {
  space: "Space",
  enter: "Enter",
  tab: "Tab",
  backquote: "Backquote",
  "`": "Backquote",
};

/** Parses `Ctrl+Space`, `Ctrl+Shift+V`, `F9`… (`null` when invalid or modifier-only). */
export function parseHotkey(s: string): HoldHotkey | null {
  const parts = s
    .split("+")
    .map((p) => p.trim())
    .filter(Boolean);
  const keyPart = parts.pop();
  if (!keyPart) return null;
  const mods = parts.map((p) => p.toLowerCase());
  if (mods.some((m) => m !== "ctrl" && m !== "shift" && m !== "alt")) return null;
  const k = keyPart.toLowerCase();
  let code: string | null = NAMED[k] ?? null;
  if (!code && /^[a-z]$/.test(k)) code = `Key${k.toUpperCase()}`;
  if (!code && /^[0-9]$/.test(k)) code = `Digit${k}`;
  if (!code && /^f([1-9]|1[0-2])$/.test(k)) code = k.toUpperCase();
  if (!code) return null;
  return { ctrl: mods.includes("ctrl"), shift: mods.includes("shift"), alt: mods.includes("alt"), code };
}

type KeyLike = Pick<KeyboardEvent, "code" | "ctrlKey" | "metaKey" | "shiftKey" | "altKey">;

/** Whether a keydown starts the hold (`Ctrl` also matches ⌘). */
export function isHoldStart(e: KeyLike, h: HoldHotkey): boolean {
  return e.code === h.code && h.ctrl === (e.ctrlKey || e.metaKey) && h.shift === e.shiftKey && h.alt === e.altKey;
}

/** Code editor (Monaco) and terminal (xterm) surfaces that own their Ctrl chords. */
const EDITOR_SURFACES = ".monaco-editor, .xterm";

/**
 * Whether the hotkey must be left to the focused editor or terminal: a
 * Ctrl chord without Alt (the default `Ctrl+Space` is Monaco's "Trigger
 * Suggest" and a common shell binding) is not taken from Monaco / xterm.
 * F-keys and Alt chords still start push-to-talk everywhere.
 */
export function yieldsToEditor(target: EventTarget | null, h: HoldHotkey): boolean {
  if (!h.ctrl || h.alt) return false;
  const el = target as { closest?: (sel: string) => unknown } | null;
  return typeof el?.closest === "function" && el.closest(EDITOR_SURFACES) != null;
}

/** Whether a keyup ends the hold. */
export function isHoldEnd(e: Pick<KeyboardEvent, "code" | "key">, h: HoldHotkey): boolean {
  if (e.code === h.code) return true;
  if (h.ctrl && (e.key === "Control" || e.key === "Meta")) return true;
  if (h.shift && e.key === "Shift") return true;
  if (h.alt && e.key === "Alt") return true;
  return false;
}

/** Display keys for `<Kbd>` (`["Ctrl", "Space"]`). */
export function hotkeyKeys(s: string): string[] {
  return s
    .split("+")
    .map((p) => p.trim())
    .filter(Boolean);
}
