/**
 * Finds user-visible string literals in TSX (used by `hardcoded.test.ts`):
 * JSX text containing letters, and literal strings passed to text props
 * (`title`, `placeholder`, `aria-label`, …) or rendered as JSX children
 * (`{cond ? "On" : "Off"}`). Arguments of calls (e.g. `t("ns:key")`) are
 * never reported — keys are not UI text.
 */
import ts from "typescript";

/** Props whose string values are shown to users or read by screen readers. */
export const TEXT_PROPS: ReadonlySet<string> = new Set([
  "title",
  "placeholder",
  "aria-label",
  "aria-description",
  "aria-valuetext",
  "alt",
  "label",
  "description",
  "tooltip",
  "hint",
  "subtitle",
]);

const LETTER = /\p{L}/u;

/** `tooltip` also takes a placement (`<IconButton tooltip="right">`), which is not text. */
const TOOLTIP_SIDES: ReadonlySet<string> = new Set(["top", "bottom", "left", "right"]);

export interface HardcodedFinding {
  line: number;
  kind: "jsx-text" | "prop" | "jsx-expression";
  prop?: string;
  text: string;
}

/** String pieces of an expression that would be rendered as is. */
function literalTexts(expr: ts.Expression): string[] {
  if (ts.isStringLiteral(expr) || ts.isNoSubstitutionTemplateLiteral(expr)) return [expr.text];
  if (ts.isTemplateExpression(expr)) return [expr.head.text, ...expr.templateSpans.map((s) => s.literal.text)];
  if (ts.isParenthesizedExpression(expr)) return literalTexts(expr.expression);
  if (ts.isConditionalExpression(expr)) return [...literalTexts(expr.whenTrue), ...literalTexts(expr.whenFalse)];
  if (ts.isBinaryExpression(expr)) {
    const op = expr.operatorToken.kind;
    if (op === ts.SyntaxKind.PlusToken || op === ts.SyntaxKind.QuestionQuestionToken || op === ts.SyntaxKind.BarBarToken) {
      return [...literalTexts(expr.left), ...literalTexts(expr.right)];
    }
    if (op === ts.SyntaxKind.AmpersandAmpersandToken) return literalTexts(expr.right);
  }
  return [];
}

function isAllowed(text: string, allowed: ReadonlySet<string>): boolean {
  const trimmed = text.replace(/\s+/g, " ").trim();
  return !LETTER.test(trimmed) || allowed.has(trimmed);
}

/** Scans one TSX source. `allowedLiterals` are exact (trimmed) texts that may stay untranslated. */
export function findHardcodedStrings(
  fileName: string,
  source: string,
  allowedLiterals: ReadonlySet<string> = new Set(),
): HardcodedFinding[] {
  const sf = ts.createSourceFile(fileName, source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const out: HardcodedFinding[] = [];
  const lineOf = (n: ts.Node) => sf.getLineAndCharacterOfPosition(n.getStart(sf)).line + 1;
  const report = (node: ts.Node, kind: HardcodedFinding["kind"], texts: string[], prop?: string) => {
    for (const text of texts) {
      if (isAllowed(text, allowedLiterals)) continue;
      const f: HardcodedFinding = { line: lineOf(node), kind, text: text.replace(/\s+/g, " ").trim() };
      if (prop) f.prop = prop;
      out.push(f);
    }
  };

  const visit = (node: ts.Node): void => {
    if (ts.isJsxText(node)) {
      report(node, "jsx-text", [node.text]);
    } else if (ts.isJsxAttribute(node)) {
      const name = node.name.getText(sf);
      const init = node.initializer;
      if (TEXT_PROPS.has(name) && init) {
        let texts: string[] = [];
        if (ts.isStringLiteral(init)) texts = [init.text];
        else if (ts.isJsxExpression(init) && init.expression) texts = literalTexts(init.expression);
        if (name === "tooltip") texts = texts.filter((x) => !TOOLTIP_SIDES.has(x));
        report(node, "prop", texts, name);
      }
    } else if (
      ts.isJsxExpression(node) &&
      node.expression &&
      (ts.isJsxElement(node.parent) || ts.isJsxFragment(node.parent))
    ) {
      report(node, "jsx-expression", literalTexts(node.expression));
    }
    ts.forEachChild(node, visit);
  };
  visit(sf);
  return out;
}
