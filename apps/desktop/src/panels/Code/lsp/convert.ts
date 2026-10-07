/**
 * LSP ↔ Monaco conversions (pure; unit tested). LSP positions are 0-based
 * line / UTF-16 character; Monaco's are 1-based line / UTF-16 column.
 */

export interface LspPosition {
  line: number;
  character: number;
}
export interface LspRange {
  start: LspPosition;
  end: LspPosition;
}
export interface MonacoRangeLike {
  startLineNumber: number;
  startColumn: number;
  endLineNumber: number;
  endColumn: number;
}

export const toLspPosition = (lineNumber: number, column: number): LspPosition => ({
  line: lineNumber - 1,
  character: column - 1,
});

export const toMonacoRange = (r: LspRange): MonacoRangeLike => ({
  startLineNumber: r.start.line + 1,
  startColumn: r.start.character + 1,
  endLineNumber: r.end.line + 1,
  endColumn: r.end.character + 1,
});

/** Workspace-relative path → `file://` URI under the project root URI. */
export function toFileUri(rootUri: string, relPath: string): string {
  const root = rootUri.replace(/\/+$/, "");
  return `${root}/${relPath.split("/").map(encodeURIComponent).join("/")}`;
}

function canonical(uri: string): string {
  let s: string;
  try {
    s = decodeURIComponent(uri);
  } catch {
    s = uri;
  }
  // `file:///C:/x` vs `file:///c%3A/x` (VS Code style): compare case-insensitively on the drive.
  return s.replace(/^file:\/\/\/([a-zA-Z]):/, (_m, d: string) => `file:///${d.toLowerCase()}:`);
}

/** `file://` URI → workspace-relative path, or null when outside the root. */
export function fromFileUri(rootUri: string, uri: string): string | null {
  const root = canonical(rootUri).replace(/\/+$/, "");
  const u = canonical(uri);
  const windowsDrive = /^file:\/\/\/[a-z]:/.test(root);
  const prefix = `${root}/`;
  const hit = windowsDrive ? u.toLowerCase().startsWith(prefix.toLowerCase()) : u.startsWith(prefix);
  if (!hit) return null;
  return u.slice(prefix.length);
}

/** LSP severity (1 Error … 4 Hint) → Monaco MarkerSeverity (8, 4, 2, 1). */
export function markerSeverity(sev: number | undefined): number {
  switch (sev) {
    case 1:
      return 8;
    case 2:
      return 4;
    case 3:
      return 2;
    case 4:
      return 1;
    default:
      return 8;
  }
}

/** LSP CompletionItemKind (1-based) → Monaco CompletionItemKind. */
export function completionKind(kind: number | undefined): number {
  // Monaco: Method 0, Function 1, Constructor 2, Field 3, Variable 4, Class 5, Struct 6, Interface 7,
  // Module 8, Property 9, Event 10, Operator 11, Unit 12, Value 13, Constant 14, Enum 15, EnumMember 16,
  // Keyword 17, Text 18, Color 19, File 20, Reference 21, Customcolor 22, Folder 23, TypeParameter 24, User 25, Issue 26, Snippet 27
  const map: Record<number, number> = {
    1: 18, // Text
    2: 0, // Method
    3: 1, // Function
    4: 2, // Constructor
    5: 3, // Field
    6: 4, // Variable
    7: 5, // Class
    8: 7, // Interface
    9: 8, // Module
    10: 9, // Property
    11: 12, // Unit
    12: 13, // Value
    13: 15, // Enum
    14: 17, // Keyword
    15: 27, // Snippet
    16: 19, // Color
    17: 20, // File
    18: 21, // Reference
    19: 23, // Folder
    20: 16, // EnumMember
    21: 14, // Constant
    22: 6, // Struct
    23: 10, // Event
    24: 11, // Operator
    25: 24, // TypeParameter
  };
  return kind !== undefined ? map[kind] ?? 18 : 18;
}

/** Markdown / plain text of LSP `MarkupContent | MarkedString | MarkedString[]`. */
export function markupToMarkdown(v: unknown): string {
  if (v === null || v === undefined) return "";
  if (typeof v === "string") return v;
  if (Array.isArray(v)) return v.map(markupToMarkdown).filter(Boolean).join("\n\n");
  if (typeof v === "object") {
    const o = v as { kind?: string; value?: string; language?: string };
    if (typeof o.value === "string") {
      if (o.language) return `\`\`\`${o.language}\n${o.value}\n\`\`\``;
      if (o.kind === "plaintext") return o.value.replace(/([\\`*_{}[\]()#+\-.!|<>])/g, "\\$1");
      return o.value;
    }
  }
  return "";
}

/** Applies LSP text edits (sorted, non-overlapping) to a string — used by tests. */
export function applyEdits(text: string, edits: readonly { range: LspRange; newText: string }[]): string {
  const lines = text.split("\n");
  const offset = (p: LspPosition): number => {
    let o = 0;
    for (let i = 0; i < p.line && i < lines.length; i++) o += (lines[i]?.length ?? 0) + 1;
    return o + p.character;
  };
  const sorted = [...edits].sort((a, b) => offset(b.range.start) - offset(a.range.start));
  let out = text;
  for (const e of sorted) out = out.slice(0, offset(e.range.start)) + e.newText + out.slice(offset(e.range.end));
  return out;
}
