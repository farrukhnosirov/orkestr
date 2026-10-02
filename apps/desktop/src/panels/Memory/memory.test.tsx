import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { parseAlternatives } from "./DecisionLog";
import { ALL_KINDS, GLOBAL_KINDS, KIND_INFO, parseTags, PROJECT_KINDS, relativeTime } from "./kinds";
import { isSafeUrl, Markdown, parseBlocks, splitHighlights } from "./markdown";

describe("markdown", () => {
  it("parses blocks", () => {
    const blocks = parseBlocks("# Title\n\nPara one\ncontinues\n\n- a\n- b\n\n1. x\n2. y\n\n> quote\n\n```rust\nfn x() {}\n# not heading\n```\n");
    expect(blocks.map((b) => b.type)).toEqual(["heading", "paragraph", "list", "list", "quote", "code"]);
    expect(blocks[1]).toEqual({ type: "paragraph", text: "Para one continues" });
    expect(blocks[3]).toMatchObject({ ordered: true, items: ["x", "y"] });
    expect(blocks[5]).toEqual({ type: "code", lang: "rust", text: "fn x() {}\n# not heading" });
  });

  it("never injects raw HTML", () => {
    const { container } = render(<Markdown source={'<img src=x onerror="alert(1)"> **bold** <script>alert(1)</script>'} />);
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector("script")).toBeNull();
    expect(container.querySelector("strong")?.textContent).toBe("bold");
    expect(container.textContent).toContain("<script>");
  });

  it("renders only http(s) links as anchors", () => {
    const { container } = render(<Markdown source="[ok](https://example.com) [bad](javascript:alert(1)) `code` _em_" />);
    const anchors = container.querySelectorAll("a");
    expect(anchors).toHaveLength(1);
    expect(anchors[0]?.getAttribute("href")).toBe("https://example.com");
    expect(anchors[0]?.getAttribute("rel")).toContain("noopener");
    expect(container.querySelector("code")?.textContent).toBe("code");
    expect(container.querySelector("em")?.textContent).toBe("em");
    expect(isSafeUrl("javascript:alert(1)")).toBe(false);
    expect(isSafeUrl("http://x.y/z")).toBe(true);
  });

  it("splits FTS highlights", () => {
    expect(splitHighlights("a \u0002b\u0003 c")).toEqual([
      { text: "a ", mark: false },
      { text: "b", mark: true },
      { text: " c", mark: false },
    ]);
    expect(splitHighlights("plain")).toEqual([{ text: "plain", mark: false }]);
  });
});

describe("memory helpers", () => {
  it("covers every kind", () => {
    expect(ALL_KINDS).toHaveLength(16);
    for (const k of [...PROJECT_KINDS, ...GLOBAL_KINDS]) expect(KIND_INFO[k]).toBeDefined();
  });

  it("parses tags", () => {
    expect(parseTags("rust, #DB, rust,, ui\nx")).toEqual(["rust", "DB", "ui", "x"]);
  });

  it("formats relative time", () => {
    const now = Date.parse("2026-10-02T12:00:00Z");
    expect(relativeTime("2026-10-02T11:59:30Z", now)).toBe("just now");
    expect(relativeTime("2026-10-02T11:00:00Z", now)).toBe("1h ago");
    expect(relativeTime("2026-09-30T12:00:00Z", now)).toBe("2d ago");
    expect(relativeTime("nope", now)).toBe("");
  });

  it("parses decision alternatives", () => {
    expect(parseAlternatives("- Tantivy: extra index\nPostgres\n\n")).toEqual([
      { title: "Tantivy", summary: "", proposedBy: null, rejectedReason: "extra index" },
      { title: "Postgres", summary: "", proposedBy: null, rejectedReason: null },
    ]);
  });
});
