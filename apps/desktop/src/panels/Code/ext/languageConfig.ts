/**
 * VS Code `language-configuration.json` → Monaco `LanguageConfiguration`
 * (pure; unit tested). Regular expressions arrive as strings or
 * `{ pattern, flags }` objects; invalid ones are dropped, never evaluated as
 * code.
 */
import type * as Monaco from "monaco-editor";

type Json = unknown;

function isObj(v: Json): v is Record<string, Json> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

/** Builds a RegExp from a VS Code regex value (`"..."` or `{ pattern, flags }`). */
export function toRegExp(v: Json): RegExp | undefined {
  let pattern: string | undefined;
  let flags = "";
  if (typeof v === "string") pattern = v;
  else if (isObj(v) && typeof v.pattern === "string") {
    pattern = v.pattern;
    if (typeof v.flags === "string") flags = v.flags.replace(/[^gimsuy]/g, "");
  }
  if (pattern === undefined || pattern.length > 10_000) return undefined;
  try {
    return new RegExp(pattern, flags);
  } catch {
    return undefined;
  }
}

function pair(v: Json): [string, string] | undefined {
  if (Array.isArray(v) && typeof v[0] === "string" && typeof v[1] === "string") return [v[0], v[1]];
  return undefined;
}

function autoPair(v: Json): Monaco.languages.IAutoClosingPairConditional | undefined {
  const p = pair(v);
  if (p) return { open: p[0], close: p[1] };
  if (isObj(v) && typeof v.open === "string" && typeof v.close === "string") {
    const out: Monaco.languages.IAutoClosingPairConditional = { open: v.open, close: v.close };
    if (Array.isArray(v.notIn)) out.notIn = v.notIn.filter((x): x is string => typeof x === "string");
    else if (typeof v.notIn === "string") out.notIn = [v.notIn];
    return out;
  }
  return undefined;
}

/** IndentAction numbers of Monaco (None 0, Indent 1, IndentOutdent 2, Outdent 3). */
const INDENT: Record<string, number> = { none: 0, indent: 1, indentoutdent: 2, outdent: 3 };

/** Converts a parsed language configuration. */
export function toMonacoLanguageConfig(cfg: Json): Monaco.languages.LanguageConfiguration {
  const out: Monaco.languages.LanguageConfiguration = {};
  if (!isObj(cfg)) return out;
  if (isObj(cfg.comments)) {
    const c = cfg.comments;
    const comments: Monaco.languages.CommentRule = {};
    if (typeof c.lineComment === "string") comments.lineComment = c.lineComment;
    else if (isObj(c.lineComment) && typeof c.lineComment.comment === "string") comments.lineComment = c.lineComment.comment;
    const block = pair(c.blockComment);
    if (block) comments.blockComment = block;
    out.comments = comments;
  }
  if (Array.isArray(cfg.brackets)) {
    out.brackets = cfg.brackets.map(pair).filter((p): p is [string, string] => p !== undefined);
  }
  if (Array.isArray(cfg.autoClosingPairs)) {
    out.autoClosingPairs = cfg.autoClosingPairs.map(autoPair).filter((p): p is Monaco.languages.IAutoClosingPairConditional => p !== undefined);
  }
  if (Array.isArray(cfg.surroundingPairs)) {
    out.surroundingPairs = cfg.surroundingPairs
      .map(autoPair)
      .filter((p): p is Monaco.languages.IAutoClosingPairConditional => p !== undefined)
      .map((p) => ({ open: p.open, close: p.close }));
  }
  if (typeof cfg.autoCloseBefore === "string") out.autoCloseBefore = cfg.autoCloseBefore;
  const word = toRegExp(cfg.wordPattern);
  if (word) out.wordPattern = word;
  if (isObj(cfg.folding)) {
    const f = cfg.folding;
    const folding: Monaco.languages.FoldingRules = {};
    if (typeof f.offSide === "boolean") folding.offSide = f.offSide;
    if (isObj(f.markers)) {
      const start = toRegExp(f.markers.start);
      const end = toRegExp(f.markers.end);
      if (start && end) folding.markers = { start, end };
    }
    out.folding = folding;
  }
  if (isObj(cfg.indentationRules)) {
    const r = cfg.indentationRules;
    const inc = toRegExp(r.increaseIndentPattern);
    const dec = toRegExp(r.decreaseIndentPattern);
    if (inc && dec) {
      const rules: Monaco.languages.IndentationRule = { increaseIndentPattern: inc, decreaseIndentPattern: dec };
      const next = toRegExp(r.indentNextLinePattern);
      if (next) rules.indentNextLinePattern = next;
      const un = toRegExp(r.unIndentedLinePattern);
      if (un) rules.unIndentedLinePattern = un;
      out.indentationRules = rules;
    }
  }
  if (Array.isArray(cfg.onEnterRules)) {
    const rules: Monaco.languages.OnEnterRule[] = [];
    for (const r of cfg.onEnterRules) {
      if (!isObj(r) || !isObj(r.action)) continue;
      const beforeText = toRegExp(r.beforeText);
      const kind = typeof r.action.indent === "string" ? INDENT[r.action.indent.toLowerCase()] : undefined;
      if (!beforeText || kind === undefined) continue;
      const action: Monaco.languages.EnterAction = { indentAction: kind };
      if (typeof r.action.appendText === "string") action.appendText = r.action.appendText;
      if (typeof r.action.removeText === "number") action.removeText = r.action.removeText;
      const rule: Monaco.languages.OnEnterRule = { beforeText, action };
      const afterText = toRegExp(r.afterText);
      if (afterText) rule.afterText = afterText;
      const previousLineText = toRegExp(r.previousLineText);
      if (previousLineText) rule.previousLineText = previousLineText;
      rules.push(rule);
    }
    out.onEnterRules = rules;
  }
  return out;
}
