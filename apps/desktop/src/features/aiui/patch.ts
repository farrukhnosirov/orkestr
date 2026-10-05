/**
 * Client-side mirror of the UI patch rules (§29 AI-editable UI, ADR-0015).
 *
 * The core is the only trusted validator (`ui.previewPatch` /
 * `ui.applyPatch`, `orkestr_protocol::ui_patch`). This module repeats the
 * cheap checks — pointer syntax, the per-actor path allowlist and the
 * "no markup, no code" text rule — so the UI can refuse obviously bad
 * patches before a round trip, and it turns the core's change list into a
 * structured, translatable summary for the preview card. Pure functions,
 * unit-tested in `aiui.test.ts`.
 */
import type { JsonValue, PanelSide, UiChange, UiPatchActor, UiPatchOp } from "@orkestr/protocol";

/** Max operations in one patch (mirrors `MAX_PATCH_OPS`). */
export const MAX_PATCH_OPS = 64;
/** Max length of one string inside a value (mirrors `MAX_PATCH_STRING_LEN`). */
export const MAX_PATCH_STRING_LEN = 512;

export interface PatchIssue {
  /** Index of the operation (or -1 for the whole patch). */
  op: number;
  code: "tooMany" | "empty" | "badPointer" | "forbiddenPath" | "missingValue" | "unsafeText";
}

/** Parses an RFC 6901 JSON pointer; `null` when it is malformed. */
export function parsePointer(path: string): string[] | null {
  if (path === "") return [];
  if (!path.startsWith("/") || path.length > 512) return null;
  const segs: string[] = [];
  for (const raw of path.slice(1).split("/")) {
    if (/~[^01]|~$/.test(raw)) return null;
    segs.push(raw.replace(/~1/g, "/").replace(/~0/g, "~"));
  }
  return segs;
}

export function isAiActor(actor: UiPatchActor): boolean {
  return actor === "Manager" || actor === "Agent";
}

/**
 * Path allowlist: the document root and `schemaVersion` are never writable;
 * AI actors may not touch the raw docking layout (only the preset).
 */
export function isPathAllowed(actor: UiPatchActor, segs: readonly string[]): boolean {
  const [a, b, c, ...rest] = segs;
  if (isAiActor(actor)) {
    if (a === "theme" && b === "tokens") return c !== undefined && rest.length === 0;
    if (a === "theme" && b === "effects") return rest.length === 0;
    if (a === "theme" && (b === "mode" || b === "name")) return c === undefined;
    if (a === "layout" && b === "preset") return c === undefined;
    if (a === "performanceMode") return b === undefined;
    return a === "prefs";
  }
  if (a === "theme" || a === "layout" || a === "prefs") return true;
  return a === "performanceMode" && b === undefined;
}

const UNSAFE_CHARS = /[<>{};\\`\u0000-\u001f\u007f]/;
const UNSAFE_WORDS = ["url(", "expression(", "javascript:", "vbscript:", "data:", "@import", "image-set(", "attr(", "eval("];

/** Whether a string is free of markup / code-like constructs. */
export function isSafeText(s: string): boolean {
  if ([...s].length > MAX_PATCH_STRING_LEN || UNSAFE_CHARS.test(s)) return false;
  const lower = s.toLowerCase();
  return !UNSAFE_WORDS.some((w) => lower.includes(w));
}

function valueIsSafe(v: JsonValue | undefined): boolean {
  if (typeof v === "string") return isSafeText(v);
  if (Array.isArray(v)) return v.every((x) => valueIsSafe(x));
  if (v !== null && typeof v === "object") {
    return Object.entries(v).every(([k, x]) => isSafeText(k) && valueIsSafe(x));
  }
  return true;
}

/** Pre-checks a patch (the core validates again, including the schema). */
export function checkPatch(patch: readonly UiPatchOp[], actor: UiPatchActor): PatchIssue[] {
  const issues: PatchIssue[] = [];
  if (patch.length === 0) issues.push({ op: -1, code: "empty" });
  if (patch.length > MAX_PATCH_OPS) issues.push({ op: -1, code: "tooMany" });
  patch.forEach((op, i) => {
    const segs = parsePointer(op.path);
    if (!segs) {
      issues.push({ op: i, code: "badPointer" });
      return;
    }
    if (segs.length === 0 || !isPathAllowed(actor, segs)) issues.push({ op: i, code: "forbiddenPath" });
    if (op.op !== "remove" && op.value === undefined) issues.push({ op: i, code: "missingValue" });
    if (!valueIsSafe(op.value)) issues.push({ op: i, code: "unsafeText" });
  });
  return issues;
}

// ---------------------------------------------------------------------------
// Change summaries
// ---------------------------------------------------------------------------

/** A change from the core, classified for display. */
export type ChangeSummary =
  | { kind: "density"; to: string }
  | { kind: "panel"; id: string; side: PanelSide | null; hidden: boolean }
  | { kind: "panelReset"; id: string }
  | { kind: "widgetsHidden"; ids: string[] }
  | { kind: "widgetsShown"; ids: string[] }
  | { kind: "backgroundDim"; from: number; to: number }
  | { kind: "hud"; field: string; to: JsonValue | null }
  | { kind: "token"; name: string; from: string | null; to: string | null }
  | { kind: "effect"; field: string; to: JsonValue | null }
  | { kind: "themeMode"; to: string }
  | { kind: "layoutPreset"; to: string }
  | { kind: "performanceMode"; to: string }
  | { kind: "other"; path: string };

function asNumber(v: JsonValue | undefined, fallback: number): number {
  return typeof v === "number" && Number.isFinite(v) ? v : fallback;
}

function asString(v: JsonValue | undefined): string | null {
  return typeof v === "string" ? v : null;
}

function asStringList(v: JsonValue | undefined): string[] {
  return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : [];
}

function isObject(v: JsonValue | undefined): v is { [key in string]?: JsonValue } {
  return v !== null && v !== undefined && typeof v === "object" && !Array.isArray(v);
}

const SIDES: readonly PanelSide[] = ["Left", "Right", "Bottom", "Center"];

function summarizeObject(prefix: string[], before: JsonValue | undefined, after: JsonValue | undefined): ChangeSummary[] {
  // A whole container changed (e.g. `/prefs` appeared): diff its members.
  const b = isObject(before) ? before : {};
  const a = isObject(after) ? after : {};
  const keys = [...new Set([...Object.keys(b), ...Object.keys(a)])];
  return keys.flatMap((k) => {
    const bv = b[k];
    const av = a[k];
    if (JSON.stringify(bv) === JSON.stringify(av)) return [];
    return summarizeChange({ path: `${prefix.map((s) => `/${s}`).join("")}/${k}`, ...(bv === undefined ? {} : { before: bv }), ...(av === undefined ? {} : { after: av }) });
  });
}

/** Classifies one change of the core's change list (may expand into several). */
export function summarizeChange(change: UiChange): ChangeSummary[] {
  const segs = parsePointer(change.path) ?? [];
  const [a, b, c] = segs;
  const { before, after } = change;
  if (a === "prefs") {
    if (b === undefined) return summarizeObject(["prefs"], before, after);
    if (b === "density") return [{ kind: "density", to: asString(after) ?? "Comfortable" }];
    if (b === "panels") {
      if (c === undefined) return summarizeObject(["prefs", "panels"], before, after);
      if (!isObject(after)) return [{ kind: "panelReset", id: c }];
      const side = asString(after["side"]);
      return [{ kind: "panel", id: c, side: SIDES.find((s) => s === side) ?? null, hidden: after["hidden"] === true }];
    }
    if (b === "hiddenWidgets") {
      const was = asStringList(before);
      const now = asStringList(after);
      const out: ChangeSummary[] = [];
      const hidden = now.filter((id) => !was.includes(id));
      const shown = was.filter((id) => !now.includes(id));
      if (hidden.length) out.push({ kind: "widgetsHidden", ids: hidden });
      if (shown.length) out.push({ kind: "widgetsShown", ids: shown });
      return out;
    }
    if (b === "backgroundDim") return [{ kind: "backgroundDim", from: asNumber(before, 0), to: asNumber(after, 0) }];
    if (b === "hud") {
      if (c === undefined) return summarizeObject(["prefs", "hud"], before, after);
      return [{ kind: "hud", field: c, to: after ?? null }];
    }
  }
  if (a === "theme") {
    if (b === "tokens" && c !== undefined) return [{ kind: "token", name: c, from: asString(before), to: asString(after) }];
    if (b === "tokens") return summarizeObject(["theme", "tokens"], before, after);
    if (b === "effects" && c !== undefined) return [{ kind: "effect", field: c, to: after ?? null }];
    if (b === "effects") return summarizeObject(["theme", "effects"], before, after);
    if (b === "mode") return [{ kind: "themeMode", to: asString(after) ?? "Dark" }];
  }
  if (a === "layout" && b === "preset") return [{ kind: "layoutPreset", to: asString(after) ?? "Coding" }];
  if (a === "performanceMode") return [{ kind: "performanceMode", to: asString(after) ?? "Balanced" }];
  return [{ kind: "other", path: change.path }];
}

/** Classifies a whole change list. */
export function summarizeChanges(changes: readonly UiChange[]): ChangeSummary[] {
  return changes.flatMap((c) => summarizeChange(c));
}

/**
 * i18n key fragment for a panel / widget id (`missionControl.tokenChart` →
 * `missionControl_tokenChart`); dots are i18next key separators.
 */
export function idKey(id: string): string {
  return id.replace(/[^A-Za-z0-9]/g, "_");
}

/** Readable fallback for an unknown id (`missionControl.tokenChart` → `token chart`). */
export function humanizeId(id: string): string {
  const last = id.split(".").pop() ?? id;
  return last
    .replace(/[-_]/g, " ")
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .toLowerCase()
    .trim();
}

/** Whether a token value looks like a color (for the swatch in the diff). */
export function isColorValue(v: string | null): boolean {
  return v !== null && /^(#[0-9a-f]{3,8}|rgba?\(|hsla?\()/i.test(v.trim());
}
