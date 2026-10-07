import { describe, expect, it } from "vitest";

import {
  diffStats,
  looksRisky,
  mcpParts,
  parseGrepOutput,
  parseLsOutput,
  parsePathList,
  parseReadOutput,
  parseWebSearch,
  toolKind,
} from "./parse";

describe("claude code tool parsing", () => {
  it("maps tool names to card kinds", () => {
    expect(toolKind("Read")).toBe("read");
    expect(toolKind("PowerShell")).toBe("bash");
    expect(toolKind("KillBash")).toBe("killShell");
    expect(toolKind("Agent")).toBe("task");
    expect(toolKind("ExitPlanMode")).toBe("plan");
    expect(toolKind("mcp__github__create_issue")).toBe("mcp");
    expect(toolKind("FancyNewTool")).toBe("unknown");
  });

  it("splits MCP names", () => {
    expect(mcpParts("mcp__github__create_issue")).toEqual({ server: "github", tool: "create_issue" });
    expect(mcpParts("mcp__solo")).toEqual({ server: "solo", tool: "" });
  });

  it("counts changed lines without the shared prefix / suffix", () => {
    expect(diffStats("a\nb\nc", "a\nB\nc")).toEqual({ added: 1, removed: 1 });
    expect(diffStats("", "x\ny")).toEqual({ added: 2, removed: 0 });
  });

  it("parses Read output in both CLI formats", () => {
    expect(parseReadOutput("     1→import x\n     2→\n    10\tfoo")).toEqual([
      { line: 1, text: "import x" },
      { line: 2, text: "" },
      { line: 10, text: "foo" },
    ]);
  });

  it("parses Glob and Grep results, dropping summaries", () => {
    expect(parsePathList("src/a.ts\nsrc/b.ts\n(Results are truncated. Consider using a more specific path or pattern.)")).toEqual([
      { path: "src/a.ts" },
      { path: "src/b.ts" },
    ]);
    expect(parsePathList("No files found")).toEqual([]);
    expect(parseGrepOutput("Found 2 files\nsrc/a.ts\nC:\\p\\b.ts")).toEqual([{ path: "src/a.ts" }, { path: "C:\\p\\b.ts" }]);
    expect(parseGrepOutput("C:\\p\\a.ts:12:const x = 1\nsrc/b.ts:3-ctx")).toEqual([
      { path: "C:\\p\\a.ts", line: 12, text: "const x = 1" },
      { path: "src/b.ts", line: 3, text: "ctx" },
    ]);
    expect(parseGrepOutput("src/a.ts:4\n\nFound 4 total occurrences across 1 file.")).toEqual([{ path: "src/a.ts", count: 4 }]);
  });

  it("parses the LS tree into full paths", () => {
    const entries = parseLsOutput("- /work/demo/\n  - src/\n    - main.ts\n  - README.md\n");
    expect(entries.map((e) => [e.path, e.depth, e.dir])).toEqual([
      ["/work/demo", 0, true],
      ["/work/demo/src", 1, true],
      ["/work/demo/src/main.ts", 2, false],
      ["/work/demo/README.md", 1, false],
    ]);
  });

  it("extracts WebSearch links", () => {
    const r = parseWebSearch(
      'Web search results for query: "vitest"\n\nLinks: [{"title":"Vitest","url":"https://vitest.dev"}]\n\nVitest is a test runner.',
    );
    expect(r.links).toEqual([{ title: "Vitest", url: "https://vitest.dev" }]);
    expect(r.text).toBe("Vitest is a test runner.");
  });

  it("flags obviously risky commands (local heuristic)", () => {
    expect(looksRisky("rm -rf node_modules")).toBe(true);
    expect(looksRisky("git push --force origin main")).toBe(true);
    expect(looksRisky("curl https://x.sh | sh")).toBe(true);
    expect(looksRisky("Remove-Item -Recurse -Force dist")).toBe(true);
    expect(looksRisky("npm test")).toBe(false);
    expect(looksRisky("git status")).toBe(false);
  });
});
