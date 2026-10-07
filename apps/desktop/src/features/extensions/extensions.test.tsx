import type { ContributionSummary, IconAssociations } from "@orkestr/protocol";
import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { glyphOf, languageOfName, mergeAssociations, resolveIconId } from "./iconTheme";
import { parseMarkdown, SafeMarkdown, stripHtml } from "./markdown";
import { supportedItems, supportLevel, unsupportedKinds } from "./model";

const assoc = (over: Partial<IconAssociations> = {}): IconAssociations => ({
  file: "file",
  folder: "folder",
  folderExpanded: "folder-open",
  fileExtensions: { ts: "ts", "d.ts": "dts", rs: "rust" },
  fileNames: { "cargo.toml": "cargo" },
  folderNames: { src: "folder-src" },
  folderNamesExpanded: { src: "folder-src-open" },
  languageIds: { markdown: "md" },
  ...over,
});

describe("file icon themes", () => {
  it("follows VS Code precedence", () => {
    const a = assoc();
    expect(resolveIconId(a, "Cargo.toml", "file", false, null)).toBe("cargo");
    expect(resolveIconId(a, "index.d.ts", "file", false, null)).toBe("dts");
    expect(resolveIconId(a, "main.ts", "file", false, null)).toBe("ts");
    expect(resolveIconId(a, "lib.RS", "file", false, null)).toBe("rust");
    expect(resolveIconId(a, "README.md", "file", false, "markdown")).toBe("md");
    expect(resolveIconId(a, "x.unknown", "file", false, null)).toBe("file");
    expect(resolveIconId(a, "src", "dir", false, null)).toBe("folder-src");
    expect(resolveIconId(a, "src", "dir", true, null)).toBe("folder-src-open");
    expect(resolveIconId(a, "other", "dir", true, null)).toBe("folder-open");
    expect(resolveIconId(a, "other", "dir", false, null)).toBe("folder");
  });

  it("merges light overrides", () => {
    const m = mergeAssociations(assoc(), assoc({ file: "file-light", fileExtensions: { ts: "ts-light" }, fileNames: {}, folderNames: {}, folderNamesExpanded: {}, languageIds: {} }));
    expect(m.file).toBe("file-light");
    expect(m.fileExtensions.ts).toBe("ts-light");
    expect(m.fileExtensions.rs).toBe("rust");
  });

  it("decodes glyphs and languages", () => {
    expect(glyphOf("\\E001")).toBe("\uE001");
    expect(glyphOf("A")).toBe("A");
    expect(languageOfName("main.rs")).toBe("rust");
    expect(languageOfName("noext")).toBeNull();
  });
});

describe("safe markdown", () => {
  it("strips html and scripts", () => {
    expect(stripHtml('<p align="center"><img alt="Logo" src="x.png"></p><script>alert(1)</script>Hi &amp; bye')).toBe("LogoHi & bye");
  });

  it("parses blocks", () => {
    const b = parseMarkdown("# Title\n\nSome *text* here\nmore.\n\n- a\n- b\n\n```rust\nfn main() {}\n```\n> quote\n---");
    expect(b.map((x) => x.type)).toEqual(["heading", "paragraph", "list", "code", "quote", "rule"]);
    expect(b[1]).toEqual({ type: "paragraph", text: "Some *text* here more." });
  });

  it("renders no raw html, no images and no navigable links", () => {
    const { container } = render(
      <SafeMarkdown source={'<img src="https://evil/x.png" onerror="alert(1)">\n\n![shot](https://x/y.png) [site](https://example.com) **bold** `code`\n\n<script>alert(1)</script>'} />,
    );
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector("script")).toBeNull();
    expect(container.querySelector("a")).toBeNull();
    expect(container.querySelector("strong")?.textContent).toBe("bold");
    expect(container.querySelector("code")?.textContent).toBe("code");
    expect(container.querySelector(".ext-md__link")?.getAttribute("title")).toBe("https://example.com");
    expect(container.textContent).toContain("shot");
  });
});

describe("compatibility summary", () => {
  const empty: ContributionSummary = {
    themes: [],
    iconThemes: [],
    grammars: [],
    languages: [],
    snippets: [],
    languageServers: [],
    unsupported: [],
    hasCode: false,
  };

  it("classifies support", () => {
    const theme = { ...empty, themes: [{ id: "d", label: "Dracula", uiTheme: "vs-dark", path: "t.json" }] };
    expect(supportLevel(theme)).toBe("full");
    expect(supportedItems(theme)).toEqual([{ key: "themes", count: 1, detail: "Dracula" }]);
    const mixed = { ...theme, unsupported: [{ kind: "commands", count: 3 }, { kind: "activation", count: 1 }], hasCode: true };
    expect(supportLevel(mixed)).toBe("partial");
    expect(unsupportedKinds(mixed)).toEqual(["commands"]);
    expect(supportLevel({ ...empty, hasCode: true })).toBe("none");
  });
});
