/**
 * Layout-independent keyboard shortcut matching (§54).
 *
 * Chromium / WebView2 report the active layout's character in `e.key` even
 * with Ctrl held, so on a Cyrillic layout Ctrl+K arrives as `key: "л"`. For
 * such non-Latin characters the shortcut key is taken from the physical key
 * (`e.code`: `KeyK`, `Digit1`, `Comma`, …). Latin layouts (QWERTY, AZERTY,
 * Dvorak) keep matching on `e.key`, so a shortcut follows the key label.
 * Pure, unit-tested.
 */

type KeyEventLike = Pick<KeyboardEvent, "key" | "code">;
type ShortcutEventLike = Pick<KeyboardEvent, "key" | "code" | "ctrlKey" | "metaKey" | "shiftKey" | "altKey">;

const PUNCTUATION_CODES: Readonly<Record<string, string>> = {
  Backquote: "`",
  Minus: "-",
  Equal: "=",
  BracketLeft: "[",
  BracketRight: "]",
  Backslash: "\\",
  Semicolon: ";",
  Quote: "'",
  Comma: ",",
  Period: ".",
  Slash: "/",
};

/** The key a physical `KeyboardEvent.code` stands for on a US layout (`KeyK` → `k`), or `null`. */
export function codeKey(code: string): string | null {
  const letter = /^Key([A-Z])$/.exec(code);
  if (letter?.[1]) return letter[1].toLowerCase();
  const digit = /^(?:Digit|Numpad)([0-9])$/.exec(code);
  if (digit?.[1]) return digit[1];
  return PUNCTUATION_CODES[code] ?? null;
}

/**
 * The lower-cased shortcut key of an event: `e.key` for named keys and Latin
 * characters, the physical key for characters of a non-Latin layout.
 */
export function shortcutKey(e: KeyEventLike): string {
  const key = e.key;
  // Named keys ("Escape", "F11", "Backspace") and printable ASCII stay as they are.
  if (key.length !== 1 || key.charCodeAt(0) < 0x80) return key.toLowerCase();
  return codeKey(e.code) ?? key.toLowerCase();
}

/**
 * Whether a keyboard event matches a shortcut string like `Ctrl+Shift+T` or
 * `Ctrl+\``. `Ctrl` also matches ⌘ on macOS. Modifiers must match exactly.
 */
export function matchesShortcut(e: ShortcutEventLike, shortcut: string): boolean {
  const parts = shortcut.split("+").map((p) => p.trim().toLowerCase());
  const key = parts.pop();
  if (!key) return false;
  const want = { ctrl: parts.includes("ctrl"), shift: parts.includes("shift"), alt: parts.includes("alt") };
  if (want.ctrl !== (e.ctrlKey || e.metaKey) || want.shift !== e.shiftKey || want.alt !== e.altKey) return false;
  return keyMatches(e, key);
}

const PHYSICAL_PUNCTUATION: Readonly<Record<string, string>> = Object.fromEntries(
  Object.entries(PUNCTUATION_CODES).map(([code, ch]) => [ch, code]),
);

/** Physical `KeyboardEvent.code` of a shortcut key (`k` → `KeyK`, `,` → `Comma`), or null for named keys. */
export function physicalCode(key: string): string | null {
  const k = key.toLowerCase();
  if (/^[a-z]$/.test(k)) return `Key${k.toUpperCase()}`;
  if (/^[0-9]$/.test(k)) return `Digit${k}`;
  return PHYSICAL_PUNCTUATION[k] ?? null;
}

/**
 * Whether the event's main key is `key` (lower-case shortcut key such as
 * `k`, `,`, `` ` ``, `f11`, `backspace`), ignoring modifiers. The backquote
 * key is matched physically on every layout (Ctrl+` toggles the terminal).
 */
export function keyMatches(e: KeyEventLike, key: string): boolean {
  const want = key.toLowerCase();
  if (want === "`") return e.key === "`" || e.code === "Backquote";
  return shortcutKey(e) === want;
}
