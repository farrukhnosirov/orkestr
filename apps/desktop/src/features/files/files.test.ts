import { OrkestrError } from "@orkestr/client";
import type { FileChangedPayload, FileEntry } from "@orkestr/protocol";
import { beforeEach, describe, expect, it } from "vitest";

import { buildTheme, cssColorToHex, withAlpha } from "../../panels/Code/theme";
import { useEditorStore } from "../../store/editor";
import { segmentsFromPositions, segmentsFromRange, splitPathPositions } from "./highlight";
import { fileIcon } from "./icons";
import { changedLines, toRanges } from "./lineDiff";
import { ancestry, basename, dirname, extension, isWithin, joinPath, movePath, validateName } from "./paths";
import { errorInfo } from "./rpc";
import { type DirState, dirsToRefresh, flattenTree, pruneDirs, updateExpanded } from "./tree";

function entry(path: string, kind: "file" | "dir" = "file"): FileEntry {
  return {
    name: basename(path),
    path,
    kind,
    symlink: false,
    size: 0,
    modified: null,
    ignored: false,
    hidden: basename(path).startsWith("."),
    readonly: false,
  };
}

function change(path: string, kind: FileChangedPayload["change"], oldPath: string | null = null): FileChangedPayload {
  return { path, oldPath, change: kind, actor: "external", agentId: null };
}

describe("paths", () => {
  it("splits and joins workspace paths", () => {
    expect(basename("a/b/c.ts")).toBe("c.ts");
    expect(basename("top")).toBe("top");
    expect(dirname("a/b/c.ts")).toBe("a/b");
    expect(dirname("top")).toBe("");
    expect(joinPath("", "x")).toBe("x");
    expect(joinPath("a", "b\\c/")).toBe("a/b/c");
    expect(isWithin("a/b", "a")).toBe(true);
    expect(isWithin("ab", "a")).toBe(false);
    expect(isWithin("anything", "")).toBe(true);
    expect(movePath("a/b/c", "a/b", "x")).toBe("x/c");
    expect(movePath("a/bc", "a/b", "x")).toBe("a/bc");
    expect(ancestry("a/b/c")).toEqual([
      { name: "a", path: "a" },
      { name: "b", path: "a/b" },
      { name: "c", path: "a/b/c" },
    ]);
    expect(extension("x.TS")).toBe("ts");
    expect(extension(".env")).toBe("");
  });

  it("validates names like the core", () => {
    expect(validateName("main.rs")).toBeNull();
    expect(validateName("src/new.ts")).toBeNull();
    for (const bad of ["", "  ", "..", "a:b", "a?", "trailing.", "dir /x", "CON", "nul.txt", "a/../b"]) {
      expect(validateName(bad), bad).not.toBeNull();
    }
  });
});

describe("lineDiff", () => {
  it("finds changed and inserted lines", () => {
    expect(changedLines("a\nb\nc", "a\nb\nc")).toEqual([]);
    expect(changedLines("a\nb\nc", "a\nB\nc")).toEqual([2]);
    expect(changedLines("a\nb\nc", "a\nb\nx\ny\nc")).toEqual([3, 4]);
    expect(changedLines("a\nb\nc", "z\na\nb\nc")).toEqual([1]);
    expect(changedLines("a\r\nb\r\n", "a\nb\nc\n")).toEqual([3]);
  });

  it("marks the deletion point for pure deletions", () => {
    expect(changedLines("a\nb\nc\nd", "a\nd")).toEqual([2]);
    expect(changedLines("a\nb", "a")).toEqual([1]);
  });

  it("handles mixed edits in the middle", () => {
    const prev = ["1", "2", "3", "4", "5", "6"].join("\n");
    const next = ["1", "2x", "3", "5", "new", "6"].join("\n");
    expect(changedLines(prev, next)).toEqual([2, 4, 5]);
  });

  it("collapses ranges", () => {
    expect(toRanges([1, 2, 3, 7, 9, 10])).toEqual([
      [1, 3],
      [7, 7],
      [9, 10],
    ]);
    expect(toRanges([])).toEqual([]);
  });
});

describe("tree", () => {
  const dirs = new Map<string, DirState>([
    ["", { status: "loaded", entries: [entry("src", "dir"), entry("docs", "dir"), entry("README.md")], truncated: false }],
    ["src", { status: "loaded", entries: [entry("src/lib", "dir"), entry("src/main.rs")], truncated: false }],
    ["src/lib", { status: "loading" }],
    ["docs", { status: "loaded", entries: [], truncated: false }],
  ]);

  it("flattens expanded folders with depth and status rows", () => {
    const rows = flattenTree(dirs, new Set(["src", "src/lib", "docs"]));
    const shape = rows.map((r) => (r.type === "entry" ? `${r.depth}:${r.entry.path}` : `${r.depth}:${r.status}`));
    expect(shape).toEqual(["0:src", "1:src/lib", "2:loading", "1:src/main.rs", "0:docs", "1:empty", "0:README.md"]);
    const collapsed = flattenTree(dirs, new Set());
    expect(collapsed.map((r) => (r.type === "entry" ? r.entry.path : r.status))).toEqual(["src", "docs", "README.md"]);
  });

  it("filters by name and opens ancestors", () => {
    const rows = flattenTree(dirs, new Set(), "main");
    expect(rows.map((r) => (r.type === "entry" ? `${r.entry.path}:${r.expanded}` : r.status))).toEqual(["src:true", "src/main.rs:false"]);
  });

  it("computes refreshes and prunes after changes", () => {
    const loaded = new Set(["", "src", "docs"]);
    expect(dirsToRefresh(change("src/new.rs", "Created"), loaded)).toEqual(["src"]);
    expect(dirsToRefresh(change("docs/a.md", "Renamed", "src/a.md"), loaded).sort()).toEqual(["docs", "src"]);
    expect(dirsToRefresh(change("deep/x/y.rs", "Modified"), loaded)).toEqual([]);
    expect(dirsToRefresh(change("", "Modified"), loaded).sort()).toEqual(["", "docs", "src"]);
    const pruned = pruneDirs(dirs, change("src", "Deleted"));
    expect([...pruned.keys()].sort()).toEqual(["", "docs"]);
    const moved = updateExpanded(new Set(["src", "src/lib", "docs"]), change("code", "Renamed", "src"));
    expect([...moved].sort()).toEqual(["code", "code/lib", "docs"]);
    const deleted = updateExpanded(new Set(["src", "src/lib", "docs"]), change("src", "Deleted"));
    expect([...deleted]).toEqual(["docs"]);
  });
});

describe("icons and highlighting", () => {
  it("maps names to icons with token colors", () => {
    expect(fileIcon("main.rs", "file").color).toContain("--ok-color");
    expect(fileIcon("src", "dir", true).icon).not.toBe(fileIcon("src", "dir", false).icon);
    expect(fileIcon("package.json", "file")).toEqual(fileIcon("PACKAGE.JSON", "file"));
    expect(fileIcon(".env.local", "file").color).toContain("warning");
  });

  it("builds highlight segments", () => {
    expect(segmentsFromPositions("main.rs", [0, 1])).toEqual([
      { text: "ma", match: true },
      { text: "in.rs", match: false },
    ]);
    expect(segmentsFromRange("let foo = 1", 4, 7)).toEqual([
      { text: "let ", match: false },
      { text: "foo", match: true },
      { text: " = 1", match: false },
    ]);
    expect(splitPathPositions("src/main.rs", [0, 4, 5])).toEqual({
      dir: "src",
      name: "main.rs",
      dirPositions: [0],
      namePositions: [0, 1],
    });
  });
});

describe("monaco theme from tokens", () => {
  it("converts CSS colors", () => {
    expect(cssColorToHex("#abc")).toBe("#aabbcc");
    expect(cssColorToHex("#22D3EE")).toBe("#22d3ee");
    expect(cssColorToHex("rgba(34, 211, 238, 0.5)")).toBe("#22d3ee80");
    expect(cssColorToHex("rgb(4 5 13)")).toBe("#04050d");
    expect(cssColorToHex("rgba(1,2,3,1)")).toBe("#010203");
    expect(cssColorToHex("linear-gradient(red, blue)")).toBeNull();
    expect(withAlpha("#22d3ee", 0.25)).toBe("#22d3ee40");
  });

  it("builds a theme from token values with fallbacks", () => {
    const tokens: Record<string, string> = { "--ok-color-accent": "#ff0000", "--ok-color-text": "rgba(255,255,255,0.9)" };
    const theme = buildTheme((t) => tokens[t] ?? "");
    expect(theme.base).toBe("vs-dark");
    expect(theme.colors["editorCursor.foreground"]).toBe("#ff0000");
    expect(theme.colors["editor.foreground"]).toBe("#ffffffe6");
    expect(theme.rules.find((r) => r.token === "keyword")?.foreground).toBe("a78bfa");
    for (const v of Object.values(theme.colors)) expect(v).toMatch(/^#[0-9a-f]{6}([0-9a-f]{2})?$/);
  });
});

describe("editor store", () => {
  beforeEach(() => {
    useEditorStore.setState({ projectId: null, tabs: [], activePath: null, reveal: null, agentModified: {}, quickOpen: false, searchOpen: false, diff: false });
    useEditorStore.getState().setProject("p1");
  });

  it("opens, activates and closes tabs", () => {
    const s = useEditorStore.getState();
    s.openFile("a.ts");
    s.openFile("b.ts", { line: 3 });
    s.openFile("a.ts");
    let st = useEditorStore.getState();
    expect(st.tabs.map((t) => t.path)).toEqual(["a.ts", "b.ts"]);
    expect(st.activePath).toBe("a.ts");
    expect(st.reveal?.path).toBe("b.ts");
    st.closeTab("a.ts");
    st = useEditorStore.getState();
    expect(st.activePath).toBe("b.ts");
    st.closeTab("b.ts");
    expect(useEditorStore.getState().activePath).toBeNull();
  });

  it("moves tabs and agent markers on rename, marks deletions", () => {
    const s = useEditorStore.getState();
    s.openFile("src/a.ts");
    s.openFile("src/sub/b.ts");
    s.markAgentModified("src/a.ts");
    s.addAgentLines("src/a.ts", [3, 1, 3]);
    s.movePath("src", "lib");
    let st = useEditorStore.getState();
    expect(st.tabs.map((t) => t.path)).toEqual(["lib/a.ts", "lib/sub/b.ts"]);
    expect(st.activePath).toBe("lib/sub/b.ts");
    expect(Object.keys(st.agentModified)).toEqual(["lib/a.ts"]);
    expect(st.tabs[0]?.agentLines).toEqual([1, 3]);
    st.markDeleted("lib/sub");
    st = useEditorStore.getState();
    expect(st.tabs.map((t) => t.deleted)).toEqual([false, true]);
    st.clearAgentMarks();
    expect(useEditorStore.getState().tabs[0]?.agentLines).toEqual([]);
  });

  it("switching project clears tabs", () => {
    useEditorStore.getState().openFile("x");
    useEditorStore.getState().setProject("p2");
    expect(useEditorStore.getState().tabs).toEqual([]);
  });
});

describe("rpc errors", () => {
  it("decodes reasons from core errors", () => {
    const info = errorInfo(new OrkestrError("conflict", "stale", { reason: "stale", actualHash: "abc" }));
    expect(info).toMatchObject({ code: "conflict", reason: "stale" });
    expect(info.data?.actualHash).toBe("abc");
    expect(errorInfo(new Error("boom"))).toMatchObject({ code: "internal", message: "boom", reason: null });
  });
});
