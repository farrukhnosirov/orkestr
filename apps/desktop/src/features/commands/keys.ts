/**
 * Layout-independent matching of a shortcut's main key.
 *
 * `KeyboardEvent.key` is the character the active keyboard layout produces,
 * even with Ctrl held: on the Russian layout Ctrl+K arrives as `key === "л"`.
 * Latin layouts are matched by character (so Ctrl+Z follows the letter on
 * AZERTY/Dvorak, as users expect); when the layout produces a non-Latin
 * character we fall back to the physical key (`KeyboardEvent.code`), the way
 * common editors do. Pure, unit-tested.
 */

const PUNCTUATION_CODES: Record<string, string> = {
  ",": "Comma",
  ".": "Period",
  "/": "Slash",
  ";": "Semicolon",
  "'": "Quote",
  "[": "BracketLeft",
  "]": "BracketRight",
  "\\": "Backslash",
  "-": "Minus",
  "=": "Equal",
  "`": "Backquote",
};

/** Physical `KeyboardEvent.code` of a shortcut key (`k` → `KeyK`, `,` → `Comma`), or null for named keys. */
export function physicalCode(key: string): string | null {
  const k = key.toLowerCase();
  if (/^[a-z]$/.test(k)) return `Key${k.toUpperCase()}`;
  if (/^[0-9]$/.test(k)) return `Digit${k}`;
  return PUNCTUATION_CODES[k] ?? null;
}

/** A single printable ASCII character: the layout is Latin for this key. */
function isLatinChar(key: string): boolean {
  return key.length === 1 && key.charCodeAt(0) >= 0x20 && key.charCodeAt(0) <= 0x7e;
}

/**
 * Whether the event's main key is `key` (lower-case shortcut key such as
 * `k`, `,`, `` ` ``, `f11`, `backspace`).
 */
export function keyMatches(e: Pick<KeyboardEvent, "key" | "code">, key: string): boolean {
  const want = key.toLowerCase();
  if (e.key.toLowerCase() === want) return true;
  const code = physicalCode(want);
  if (!code) return false;
  // The backquote key is matched physically on every layout (Ctrl+` toggles the terminal).
  if (code === "Backquote") return e.code === code;
  return !isLatinChar(e.key) && e.code === code;
}
