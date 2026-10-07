import { describe, expect, it } from "vitest";

import { applyEdits, completionKind, fromFileUri, markerSeverity, markupToMarkdown, toFileUri, toLspPosition, toMonacoRange } from "../lsp/convert";
import { toMonacoLanguageConfig, toRegExp } from "./languageConfig";
import { bestRule, colorMapRules, fontStyleName, rawThemeFromMonaco, tmToken, toMonacoTheme, toRawTheme } from "./vscodeTheme";

const dracula = {
  uiTheme: "vs-dark",
  colors: { "editor.background": "#282a36", "editor.foreground": "#f8f8f2" },
  tokenColors: [
    { scope: ["comment"], foreground: "#6272a4", background: null, fontStyle: "italic" },
    { scope: ["keyword", "storage.type"], foreground: "#ff79c6", background: null, fontStyle: null },
    { scope: ["keyword.operator"], foreground: "#ff79c6", background: null, fontStyle: null },
    { scope: ["string"], foreground: "#f1fa8c", background: null, fontStyle: null },
    { scope: ["meta.function-call entity.name.function"], foreground: "#123456", background: null, fontStyle: null },
    { scope: ["entity.name.function"], foreground: "#50fa7b", background: null, fontStyle: null },
  ],
};

describe("VS Code theme → Monaco", () => {
  it("maps workbench colors, defaults and Monarch tokens", () => {
    const t = toMonacoTheme(dracula);
    expect(t.base).toBe("vs-dark");
    expect(t.colors["editor.background"]).toBe("#282a36");
    expect(t.rules[0]).toEqual({ token: "", foreground: "f8f8f2" });
    const rule = (token: string) => t.rules.find((r) => r.token === token);
    expect(rule("comment")).toEqual({ token: "comment", foreground: "6272a4", fontStyle: "italic" });
    expect(rule("keyword")?.foreground).toBe("ff79c6");
    expect(rule("operator")?.foreground).toBe("ff79c6");
    expect(rule("function")?.foreground).toBe("50fa7b");
    expect(toMonacoTheme({ ...dracula, uiTheme: "vs" }).base).toBe("vs");
  });

  it("picks the longest simple selector and ignores parent selectors", () => {
    expect(bestRule(dracula.tokenColors, "keyword.operator.arithmetic")?.scope).toEqual(["keyword.operator"]);
    expect(bestRule(dracula.tokenColors, "entity.name.function.rust")?.foreground).toBe("#50fa7b");
    expect(bestRule(dracula.tokenColors, "variable")).toBeNull();
  });

  it("encodes TextMate color map tokens", () => {
    expect(tmToken(3, 1)).toBe("tm.3.1");
    expect(fontStyleName(1 | 2)).toBe("italic bold");
    const rules = colorMapRules(["", "#FF0000", "#00ff00"]);
    expect(rules).toHaveLength(32);
    expect(rules).toContainEqual({ token: "tm.1.0", foreground: "FF0000" });
    expect(rules).toContainEqual({ token: "tm.2.1", foreground: "00ff00", fontStyle: "italic" });
  });

  it("builds raw themes for vscode-textmate", () => {
    const raw = toRawTheme(dracula);
    expect(raw.settings[0]).toEqual({ settings: { foreground: "#f8f8f2", background: "#282a36" } });
    expect(raw.settings[1]).toEqual({ scope: ["comment"], settings: { foreground: "#6272a4", fontStyle: "italic" } });
    const fromOrkestr = rawThemeFromMonaco({
      base: "vs-dark",
      inherit: true,
      rules: [
        { token: "comment", foreground: "888888", fontStyle: "italic" },
        { token: "function", foreground: "00aaff" },
      ],
      colors: { "editor.foreground": "#eeeeee" },
    });
    expect(fromOrkestr.settings).toContainEqual({ scope: ["comment"], settings: { foreground: "#888888", fontStyle: "italic" } });
    expect(fromOrkestr.settings).toContainEqual({ scope: ["entity.name.function"], settings: { foreground: "#00aaff" } });
  });
});

describe("language configuration", () => {
  it("converts comments, brackets, pairs, folding, indentation and onEnter", () => {
    const c = toMonacoLanguageConfig({
      comments: { lineComment: "//", blockComment: ["/*", "*/"] },
      brackets: [["{", "}"], ["(", ")"], ["bad"]],
      autoClosingPairs: [["[", "]"], { open: '"', close: '"', notIn: ["string"] }],
      surroundingPairs: [["<", ">"]],
      wordPattern: "(-?\\d*\\.\\d\\w*)|([^\\s]+)",
      folding: { markers: { start: "^\\s*//\\s*#?region\\b", end: { pattern: "^\\s*//\\s*#?endregion\\b", flags: "i" } } },
      indentationRules: { increaseIndentPattern: "^.*\\{[^}]*$", decreaseIndentPattern: "^\\s*\\}" },
      onEnterRules: [{ beforeText: "^\\s*/\\*\\*", action: { indent: "indentOutdent", appendText: " * " } }],
    });
    expect(c.comments).toEqual({ lineComment: "//", blockComment: ["/*", "*/"] });
    expect(c.brackets).toEqual([["{", "}"], ["(", ")"]]);
    expect(c.autoClosingPairs).toEqual([{ open: "[", close: "]" }, { open: '"', close: '"', notIn: ["string"] }]);
    expect(c.surroundingPairs).toEqual([{ open: "<", close: ">" }]);
    expect(c.wordPattern?.test("abc")).toBe(true);
    expect(c.folding?.markers?.end.flags).toBe("i");
    expect(c.indentationRules?.increaseIndentPattern.test("fn x() {")).toBe(true);
    expect(c.onEnterRules?.[0]?.action).toEqual({ indentAction: 2, appendText: " * " });
  });

  it("drops invalid regexes", () => {
    expect(toRegExp("(")).toBeUndefined();
    expect(toRegExp(42)).toBeUndefined();
    expect(toMonacoLanguageConfig({ wordPattern: "(" }).wordPattern).toBeUndefined();
    expect(toMonacoLanguageConfig(null)).toEqual({});
  });
});

describe("LSP conversions", () => {
  it("converts positions and ranges", () => {
    expect(toLspPosition(1, 1)).toEqual({ line: 0, character: 0 });
    expect(toMonacoRange({ start: { line: 2, character: 4 }, end: { line: 2, character: 9 } })).toEqual({
      startLineNumber: 3,
      startColumn: 5,
      endLineNumber: 3,
      endColumn: 10,
    });
  });

  it("maps file URIs both ways", () => {
    const root = "file:///C:/Users/me/my%20proj";
    const uri = toFileUri(root, "src/main file.rs");
    expect(uri).toBe("file:///C:/Users/me/my%20proj/src/main%20file.rs");
    expect(fromFileUri(root, uri)).toBe("src/main file.rs");
    // VS Code style encoding of the drive letter from a server.
    expect(fromFileUri(root, "file:///c%3A/Users/me/my%20proj/src/lib.rs")).toBe("src/lib.rs");
    expect(fromFileUri(root, "file:///C:/Users/me/other/x.rs")).toBeNull();
    expect(fromFileUri("file:///home/me/p", "file:///home/me/p/a/b.go")).toBe("a/b.go");
    expect(fromFileUri("file:///home/me/p", "file:///home/me/pp/a.go")).toBeNull();
  });

  it("maps kinds, severities and markup", () => {
    expect(markerSeverity(1)).toBe(8);
    expect(markerSeverity(2)).toBe(4);
    expect(completionKind(3)).toBe(1);
    expect(completionKind(15)).toBe(27);
    expect(markupToMarkdown({ kind: "markdown", value: "**x**" })).toBe("**x**");
    expect(markupToMarkdown({ language: "rust", value: "fn x()" })).toBe("```rust\nfn x()\n```");
    expect(markupToMarkdown(["a", { language: "go", value: "x" }])).toBe("a\n\n```go\nx\n```");
    expect(markupToMarkdown({ kind: "plaintext", value: "a*b" })).toBe("a\\*b");
  });

  it("applies text edits", () => {
    const text = "fn main() {\n    let x = 1;\n}";
    const out = applyEdits(text, [
      { range: { start: { line: 1, character: 8 }, end: { line: 1, character: 9 } }, newText: "count" },
      { range: { start: { line: 0, character: 3 }, end: { line: 0, character: 7 } }, newText: "start" },
    ]);
    expect(out).toBe("fn start() {\n    let count = 1;\n}");
  });
});
