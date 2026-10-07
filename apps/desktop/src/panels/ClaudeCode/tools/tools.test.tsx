import type { ClaudeRisk, JsonValue } from "@orkestr/protocol";
import type { ReactNode } from "react";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { i18n } from "../../../i18n";
import type { ToolResultView } from "../types";
import { ToolCard } from "./index";

// Monaco does not run in jsdom: the lazy editor chunk is replaced by a marker.
vi.mock("./MonacoDiffEditor", () => ({
  default: ({ before, after, path }: { before: string; after: string; path: string }) => (
    <div data-testid="monaco-diff" data-path={path}>
      {before}|{after}
    </div>
  ),
}));

const ROOT = "C:\\work\\demo";

function card(name: string, input: JsonValue, result: ToolResultView = null, extra: { risk?: ClaudeRisk | null; children?: ReactNode } = {}) {
  const onOpenFile = vi.fn();
  const utils = render(
    <ToolCard call={{ id: "toolu_1", name, input }} result={result} root={ROOT} cwd={ROOT} onOpenFile={onOpenFile} {...extra} />,
  );
  return { ...utils, onOpenFile };
}

const ok = (content: string): ToolResultView => ({ content, isError: false });
const err = (content: string): ToolResultView => ({ content, isError: true });

function toggle() {
  const btn = screen.getAllByRole("button", { expanded: false })[0];
  if (!btn) throw new Error("no collapsed toggle");
  fireEvent.click(btn);
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(cleanup);

describe("ToolCard", () => {
  it("Edit: collapsed header with path + stats, expands to a Monaco diff, switches to text, opens the file", async () => {
    const { onOpenFile } = card(
      "Edit",
      { file_path: "C:\\work\\demo\\src\\a.ts", old_string: "const a = 1;\nkeep", new_string: "const a = 2;\nkeep" },
      ok("The file has been updated."),
    );
    expect(screen.getByText("Edit")).toBeTruthy();
    expect(screen.getByText("src/a.ts")).toBeTruthy();
    expect(screen.getByLabelText("1 lines added")).toBeTruthy();
    expect(screen.queryByTestId("monaco-diff")).toBeNull();

    const head = screen.getByRole("button", { name: "Show details: Edit" });
    expect(head.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(head);
    expect(head.getAttribute("aria-expanded")).toBe("true");
    const monaco = await screen.findByTestId("monaco-diff");
    expect(monaco.getAttribute("data-path")).toBe("C:\\work\\demo\\src\\a.ts");

    fireEvent.click(screen.getByRole("button", { name: "Text view" }));
    expect(screen.queryByTestId("monaco-diff")).toBeNull();
    const diff = screen.getByRole("figure", { name: "Changes in C:\\work\\demo\\src\\a.ts" });
    expect(diff.querySelector(".cct-diff__line--del")?.textContent).toContain("const a = 1;");
    expect(diff.querySelector(".cct-diff__line--add")?.textContent).toContain("const a = 2;");

    fireEvent.click(screen.getByRole("button", { name: "Open file" }));
    expect(onOpenFile).toHaveBeenCalledWith("src/a.ts", undefined);
    fireEvent.click(screen.getByRole("button", { name: "Open src/a.ts" }));
    expect(onOpenFile).toHaveBeenCalledTimes(2);
  });

  it("MultiEdit: one diff per edit and a change count", async () => {
    card("MultiEdit", {
      file_path: "C:\\work\\demo\\b.rs",
      edits: [
        { old_string: "a", new_string: "b" },
        { old_string: "c", new_string: "d" },
      ],
    });
    expect(screen.getByText("2 changes")).toBeTruthy();
    expect(screen.getByRole("status", { name: "Running" })).toBeTruthy();
    toggle();
    expect(await screen.findAllByTestId("monaco-diff")).toHaveLength(2);
    expect(screen.getByText("Change 2 of 2")).toBeTruthy();
  });

  it("Write: new file diff", async () => {
    card("Write", { file_path: "C:\\work\\demo\\new.txt", content: "hello" }, ok("File created successfully"));
    toggle();
    expect(screen.getByText("New file")).toBeTruthy();
    expect((await screen.findByTestId("monaco-diff")).textContent).toBe("|hello");
  });

  it("Read: range + snippet, opens at the first line", () => {
    const { onOpenFile } = card(
      "Read",
      { file_path: "C:\\work\\demo\\src\\main.ts", offset: 10, limit: 3 },
      ok("    10→fn main() {\n    11→  run();\n    12→}"),
    );
    toggle();
    expect(screen.getByText("Lines 10–12")).toBeTruthy();
    const snippet = screen.getByRole("figure", { name: "File preview" });
    expect(within(snippet).getByText("run();", { exact: false })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Open file" }));
    expect(onOpenFile).toHaveBeenCalledWith("src/main.ts", 10);
  });

  it("Bash: command, description, output, guard risk badge and error styling", () => {
    const risk: ClaudeRisk = { level: "dangerous", category: "RecursiveDelete", reason: "recursive delete" };
    const { container } = card(
      "Bash",
      { command: "rm -rf build", description: "Clean the build folder" },
      err("Exit code 1\nrm: cannot remove 'build': Permission denied"),
      { risk },
    );
    const badge = screen.getByText(/^Dangerous:/);
    expect(badge.closest("[title]")?.getAttribute("title")).toBe("recursive delete");
    // Failed cards open by default.
    expect(screen.getByText("Clean the build folder")).toBeTruthy();
    expect(container.querySelector(".cct-command")?.textContent).toBe("rm -rf build");
    expect(container.querySelector(".cct-output--error")?.textContent).toContain("Permission denied");
    expect(screen.getByText("Failed")).toBeTruthy();
  });

  it("Bash without a guard classification uses the labelled local heuristic", () => {
    card("Bash", { command: "git push --force" }, ok(""));
    const badge = screen.getByText("Possibly risky");
    expect(badge.closest("[data-heuristic]")).toBeTruthy();
    cleanup();
    card("Bash", { command: "npm test" }, ok("all green"));
    expect(screen.queryByText("Possibly risky")).toBeNull();
  });

  it("Bash: long output is truncated with an expander", () => {
    const out = Array.from({ length: 40 }, (_, i) => `line ${i + 1}`).join("\n");
    const { container } = card("Bash", { command: "seq 40" }, ok(out));
    toggle();
    expect(container.querySelector(".cct-output")?.textContent).not.toContain("line 40");
    fireEvent.click(screen.getByRole("button", { name: "Show all 40 lines" }));
    expect(container.querySelector(".cct-output")?.textContent).toContain("line 40");
  });

  it("Grep: clickable results with line numbers", () => {
    const { onOpenFile } = card("Grep", { pattern: "TODO", output_mode: "content", "-n": true }, ok("C:\\work\\demo\\src\\a.ts:7:// TODO fix"));
    toggle();
    expect(screen.getByText("1 result")).toBeTruthy();
    fireEvent.click(screen.getByTitle("Open src/a.ts at line 7"));
    expect(onOpenFile).toHaveBeenCalledWith("src/a.ts", 7);
  });

  it("Glob: relative results resolve against the search path; empty result", () => {
    const { onOpenFile } = card("Glob", { pattern: "**/*.ts", path: "C:\\work\\demo\\pkg" }, ok("x.ts\ny.ts"));
    toggle();
    fireEvent.click(screen.getByTitle("Open pkg/y.ts"));
    expect(onOpenFile).toHaveBeenCalledWith("pkg/y.ts", undefined);
    cleanup();
    card("Glob", { pattern: "*.zz" }, ok("No files found"));
    toggle();
    expect(screen.getByText("Nothing found")).toBeTruthy();
  });

  it("LS: tree with clickable files", () => {
    const { onOpenFile } = card("LS", { path: "C:\\work\\demo" }, ok("- C:\\work\\demo\\\n  - src\\\n    - main.ts\n"));
    toggle();
    fireEvent.click(screen.getByTitle("Open src/main.ts"));
    expect(onOpenFile).toHaveBeenCalledWith("src/main.ts", undefined);
  });

  it("TodoWrite: open checklist with statuses and progress", () => {
    card("TodoWrite", {
      todos: [
        { content: "Write tests", status: "completed", activeForm: "Writing tests" },
        { content: "Fix bug", status: "in_progress", activeForm: "Fixing bug" },
        { content: "Ship", status: "pending", activeForm: "Shipping" },
      ],
    });
    expect(screen.getByText("1 of 3 done")).toBeTruthy();
    const list = screen.getByRole("list", { name: "Todo list" });
    expect(within(list).getAllByRole("listitem")).toHaveLength(3);
    expect(within(list).getByText("In progress")).toBeTruthy();
    expect(list.querySelector(".cct-todo--completed")?.textContent).toContain("Write tests");
  });

  it("WebFetch / WebSearch: url, query, sources and summary", () => {
    card("WebFetch", { url: "https://example.com/doc", prompt: "Summarize" }, ok("# Title\n\nShort summary."));
    toggle();
    expect(screen.getAllByText("https://example.com/doc").length).toBeGreaterThan(0);
    expect(screen.getByText("Summarize")).toBeTruthy();
    expect(screen.getByText("Short summary.")).toBeTruthy();
    cleanup();
    card("WebSearch", { query: "vitest" }, ok('Links: [{"title":"Vitest","url":"https://vitest.dev"}]\n\nFast tests.'));
    toggle();
    expect(screen.getByRole("list", { name: "Sources" })).toBeTruthy();
    expect(screen.getByText("Fast tests.")).toBeTruthy();
  });

  it("Task: description, agent type, nested items and result", () => {
    card(
      "Task",
      { description: "Explore repo", prompt: "Find the auth code", subagent_type: "Explore" },
      ok("Found it in **auth.rs**."),
      { children: <div data-testid="nested">nested tool</div> },
    );
    expect(screen.getByText("Explore repo")).toBeTruthy();
    expect(screen.getByText("Agent: Explore")).toBeTruthy();
    expect(within(screen.getByRole("group", { name: "Subagent steps" })).getByTestId("nested")).toBeTruthy();
    expect(screen.getByText("auth.rs")).toBeTruthy();
  });

  it("ExitPlanMode: renders the plan as safe Markdown", () => {
    const { container } = card("ExitPlanMode", { plan: "## Plan\n\n- step one\n- <script>alert(1)</script>step two" });
    expect(screen.getByRole("document", { name: "Proposed plan" })).toBeTruthy();
    expect(screen.getByText("step one")).toBeTruthy();
    expect(container.querySelector("script")).toBeNull();
  });

  it("MCP and unknown tools fall back to a JSON view", () => {
    card("mcp__github__create_issue", { title: "Bug" }, ok("created #5"));
    expect(screen.getByText("MCP · github")).toBeTruthy();
    expect(screen.getByText("create_issue")).toBeTruthy();
    toggle();
    expect(screen.getByText("Arguments")).toBeTruthy();
    expect(screen.getByText(/"title": "Bug"/)).toBeTruthy();
    cleanup();
    card("BrandNewTool", { x: 1 }, err("boom"));
    expect(screen.getByText("Tool")).toBeTruthy();
    expect(screen.getByText("boom")).toBeTruthy();
  });

  it("paths outside the project are shown but not clickable", () => {
    card("Read", { file_path: "D:\\elsewhere\\x.txt" }, ok("     1→x"));
    expect(screen.queryByRole("button", { name: /^Open / })).toBeNull();
    toggle();
    expect(screen.getAllByTitle("Outside the project").length).toBeGreaterThan(0);
  });

  it("renders in Russian", async () => {
    await i18n.changeLanguage("ru");
    card("Edit", { file_path: "C:\\work\\demo\\a.ts", old_string: "a", new_string: "b" }, err("Строка не найдена"));
    expect(screen.getByText("Правка")).toBeTruthy();
    expect(screen.getByText("Ошибка")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Открыть файл" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Текстом" })).toBeTruthy();
    cleanup();
    card("TodoWrite", { todos: [{ content: "Тест", status: "pending" }] });
    expect(screen.getByText("Выполнено 0 из 1")).toBeTruthy();
    cleanup();
    card("Bash", { command: "ls" }, null);
    expect(screen.getByText("Команда")).toBeTruthy();
    expect(screen.getByRole("status", { name: "Выполняется" })).toBeTruthy();
  });
});
