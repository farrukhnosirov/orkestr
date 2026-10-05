/**
 * Token value grammars and the strict CSS color parser. A line-by-line
 * mirror of `orkestr_protocol::ui_theme` / `ui_config` in Rust; both are
 * tested against `crates/protocol/fixtures/ui_theme/cases.json`.
 */
import type { Rgba, TokenKind } from "@orkestr/protocol";
import { MAX_TOKEN_VALUE_LEN, TOKEN_CATALOGUE } from "@orkestr/protocol";

const TOKEN_NAME_RE = /^[a-z][a-z0-9-]{0,63}$/;
const ALLOWED_VALUE_CHARS = /^[A-Za-z0-9 #%.,()\-+/'"_!]+$/;
const FORBIDDEN_SUBSTRINGS = ["url(", "expression(", "javascript:", "@import", "image-set(", "attr("];

/** `^[a-z][a-z0-9-]{0,63}$` (also used for theme ids and preset ids). */
export function isValidTokenName(name: string): boolean {
  return TOKEN_NAME_RE.test(name);
}

/** Generic safety rule for every token value. */
export function isValidTokenValue(value: string): boolean {
  if (value.length === 0 || value.length > MAX_TOKEN_VALUE_LEN) return false;
  if (!ALLOWED_VALUE_CHARS.test(value)) return false;
  const lower = value.toLowerCase();
  return !FORBIDDEN_SUBSTRINGS.some((bad) => lower.includes(bad));
}

/** Grammar of a known token, `null` for free-form names. */
export function tokenKind(name: string): TokenKind | null {
  return Object.prototype.hasOwnProperty.call(TOKEN_CATALOGUE, name) ? (TOKEN_CATALOGUE[name] ?? null) : null;
}

/** `-?(\d+(\.\d*)?|\.\d+)` — no exponents, no `Infinity`/`NaN`. */
export function plainNumber(s: string): number | null {
  const body = s.startsWith("-") ? s.slice(1) : s;
  if (body.length === 0 || !/^[0-9.]+$/.test(body)) return null;
  const dots = body.split(".").length - 1;
  if (dots > 1 || body === ".") return null;
  const v = Number(s);
  return Number.isFinite(v) ? v : null;
}

/** `var(--ok-<token>)`. */
export function isVarRef(s: string): boolean {
  const t = s.trim();
  if (!t.startsWith("var(--ok-") || !t.endsWith(")")) return false;
  return isValidTokenName(t.slice("var(--ok-".length, -1));
}

function fnArgs(s: string, name: string): string[] | null {
  if (!s.startsWith(name)) return null;
  const rest = s.slice(name.length).trimStart();
  if (!rest.startsWith("(") || !rest.endsWith(")")) return null;
  return rest
    .slice(1, -1)
    .split(/[,/\s]/)
    .filter((p) => p.length > 0);
}

function percent(s: string): number | null {
  if (!s.endsWith("%")) return null;
  const v = plainNumber(s.slice(0, -1));
  return v !== null && v >= 0 && v <= 100 ? v : null;
}

function alpha(s: string): number | null {
  if (s.endsWith("%")) {
    const p = percent(s);
    return p === null ? null : p / 100;
  }
  const v = plainNumber(s);
  return v !== null && v >= 0 && v <= 1 ? v : null;
}

function channel(s: string): number | null {
  if (s.endsWith("%")) {
    const p = percent(s);
    return p === null ? null : p * 2.55;
  }
  const v = plainNumber(s);
  return v !== null && v >= 0 && v <= 255 ? v : null;
}

/**
 * Parses `#rgb[a]`, `#rrggbb[aa]`, `rgb[a]()`, `hsl[a]()` (comma or space
 * syntax), `transparent`, `black`, `white`. Anything else → `null`.
 */
export function parseColor(input: string): Rgba | null {
  const s = input.trim().toLowerCase();
  if (s === "transparent") return { r: 0, g: 0, b: 0, a: 0 };
  if (s === "black") return { r: 0, g: 0, b: 0, a: 1 };
  if (s === "white") return { r: 255, g: 255, b: 255, a: 1 };
  if (s.startsWith("#")) {
    const hex = s.slice(1);
    if (!/^[0-9a-f]*$/.test(hex)) return null;
    const d = [...hex].map((c) => parseInt(c, 16));
    const at = (i: number) => d[i] ?? 0;
    if (d.length === 3 || d.length === 4) {
      return { r: at(0) * 17, g: at(1) * 17, b: at(2) * 17, a: d.length === 4 ? (at(3) * 17) / 255 : 1 };
    }
    if (d.length === 6 || d.length === 8) {
      return {
        r: at(0) * 16 + at(1),
        g: at(2) * 16 + at(3),
        b: at(4) * 16 + at(5),
        a: d.length === 8 ? (at(6) * 16 + at(7)) / 255 : 1,
      };
    }
    return null;
  }
  const rgb = fnArgs(s, "rgba") ?? fnArgs(s, "rgb");
  if (rgb) {
    if (rgb.length < 3 || rgb.length > 4) return null;
    const [r, g, b] = [channel(rgb[0] ?? ""), channel(rgb[1] ?? ""), channel(rgb[2] ?? "")];
    const a = rgb[3] === undefined ? 1 : alpha(rgb[3]);
    if (r === null || g === null || b === null || a === null) return null;
    return { r, g, b, a };
  }
  const hsl = fnArgs(s, "hsla") ?? fnArgs(s, "hsl");
  if (hsl) {
    if (hsl.length < 3 || hsl.length > 4) return null;
    const h0 = hsl[0] ?? "";
    const hRaw = plainNumber(h0.endsWith("deg") ? h0.slice(0, -3) : h0);
    const sat = percent(hsl[1] ?? "");
    const light = percent(hsl[2] ?? "");
    const a = hsl[3] === undefined ? 1 : alpha(hsl[3]);
    if (hRaw === null || sat === null || light === null || a === null) return null;
    const h = ((hRaw % 360) + 360) % 360;
    const sn = sat / 100;
    const ln = light / 100;
    const c = (1 - Math.abs(2 * ln - 1)) * sn;
    const hp = h / 60;
    const x = c * (1 - Math.abs((hp % 2) - 1));
    const sector = Math.floor(hp);
    const [r1, g1, b1] =
      sector === 0 ? [c, x, 0] : sector === 1 ? [x, c, 0] : sector === 2 ? [0, c, x] : sector === 3 ? [0, x, c] : sector === 4 ? [x, 0, c] : [c, 0, x];
    const m = ln - c / 2;
    return { r: (r1 + m) * 255, g: (g1 + m) * 255, b: (b1 + m) * 255, a };
  }
  return null;
}

function balanced(s: string): boolean {
  let depth = 0;
  for (const c of s) {
    if (c === "(") depth += 1;
    else if (c === ")") {
      depth -= 1;
      if (depth < 0) return false;
    }
  }
  return depth === 0;
}

/** Splits on commas that are not inside parentheses. */
export function splitTopLevel(s: string): string[] {
  const out: string[] = [];
  let depth = 0;
  let start = 0;
  for (let i = 0; i < s.length; i += 1) {
    const c = s[i];
    if (c === "(") depth += 1;
    else if (c === ")") depth -= 1;
    else if (c === "," && depth === 0) {
      out.push(s.slice(start, i));
      start = i + 1;
    }
  }
  out.push(s.slice(start));
  return out;
}

const GRADIENT_FNS = [
  "linear-gradient(",
  "radial-gradient(",
  "conic-gradient(",
  "repeating-linear-gradient(",
  "repeating-radial-gradient(",
];

function isLength(raw: string): boolean {
  const s = raw.trim();
  if (s === "0" || isVarRef(s)) return true;
  return ["px", "rem", "em", "%", "vh", "vw"].some((u) => s.endsWith(u) && plainNumber(s.slice(0, -u.length)) !== null);
}

function isFontFamily(s: string): boolean {
  if (isVarRef(s)) return true;
  const ok = /^[A-Za-z0-9 \-_.]*$/;
  return s.split(",").every((part) => {
    const p = part.trim();
    if (p.length === 0) return false;
    const quoted = (p.startsWith("'") && p.endsWith("'")) || (p.startsWith('"') && p.endsWith('"'));
    return quoted ? p.length >= 3 && ok.test(p.slice(1, -1)) : ok.test(p);
  });
}

function isDuration(raw: string): boolean {
  const s = raw.trim();
  if (isVarRef(s)) return true;
  let ms: number | null = null;
  if (s.endsWith("ms")) ms = plainNumber(s.slice(0, -2));
  else if (s.endsWith("s")) {
    const v = plainNumber(s.slice(0, -1));
    ms = v === null ? null : v * 1000;
  }
  return ms !== null && ms >= 0 && ms <= 10_000;
}

/** Parses a duration token into milliseconds (`null` for `var()` / invalid). */
export function durationMs(raw: string): number | null {
  const s = raw.trim();
  if (s.endsWith("ms")) return plainNumber(s.slice(0, -2));
  if (s.endsWith("s")) {
    const v = plainNumber(s.slice(0, -1));
    return v === null ? null : v * 1000;
  }
  return null;
}

function isEasing(raw: string): boolean {
  const s = raw.trim();
  if (isVarRef(s) || ["linear", "ease", "ease-in", "ease-out", "ease-in-out"].includes(s)) return true;
  const args = fnArgs(s, "cubic-bezier");
  if (!args || args.length !== 4) return false;
  const nums = args.map(plainNumber);
  if (nums.some((n) => n === null)) return false;
  const [x1, , x2] = nums as number[];
  return x1 !== undefined && x2 !== undefined && x1 >= 0 && x1 <= 1 && x2 >= 0 && x2 <= 1;
}

/** Whether `value` matches the grammar of `kind` (generic safety is checked separately). */
export function matchesKind(kind: TokenKind, value: string): boolean {
  const v = value.trim();
  switch (kind) {
    case "Color":
      return parseColor(v) !== null || isVarRef(v);
    case "Gradient":
      return (
        balanced(v) &&
        splitTopLevel(v).every((layer) => {
          const l = layer.trim().toLowerCase();
          return parseColor(l) !== null || isVarRef(l) || (GRADIENT_FNS.some((f) => l.startsWith(f)) && l.endsWith(")"));
        })
      );
    case "Shadow":
      return balanced(v);
    case "Length":
      return isLength(v);
    case "FontFamily":
      return isFontFamily(v);
    case "FontWeight": {
      if (isVarRef(v)) return true;
      const w = plainNumber(v);
      return w !== null && Number.isInteger(w) && w >= 100 && w <= 900;
    }
    case "Number":
      return isVarRef(v) || plainNumber(v) !== null;
    case "Duration":
      return isDuration(v);
    case "Easing":
      return isEasing(v);
  }
}

/** `#rrggbb` (or `#rrggbbaa` when translucent) for a parsed color. */
export function toHex(c: Rgba): string {
  const h = (n: number) => Math.max(0, Math.min(255, Math.round(n))).toString(16).padStart(2, "0");
  return `#${h(c.r)}${h(c.g)}${h(c.b)}${c.a < 1 ? h(c.a * 255) : ""}`;
}
