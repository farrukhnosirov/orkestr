// Claude Code chat model: event folding, nesting, helpers.
import type { ClaudeCodeEvent } from "@orkestr/protocol";
import { describe, expect, it } from "vitest";

import {
  applyAll,
  applyEvent,
  composerKey,
  composerPopup,
  editPairs,
  emptyChat,
  insertMention,
  lineDiff,
  relativeTo,
  selectionPrompt,
  todos,
  toolSubject,
  type ChatState,
} from "./model";

const usage = { inputTokens: 10, outputTokens: 5, cacheReadTokens: 100, cacheWriteTokens: 0 };

function fold(events: ClaudeCodeEvent[]): ChatState {
  return events.reduce(applyEvent, emptyChat());
}

describe("applyEvent", () => {
  it("streams text and replaces it with the complete block", () => {
    const s = fold([
      { type: "userMessage", text: "hi" },
      { type: "thinking" },
      { type: "textDelta", text: "Hel", parentToolUseId: null },
      { type: "textDelta", text: "lo", parentToolUseId: null },
    ]);
    expect(s.thinking).toBe(false);
    expect(s.running).toBe(true);
    const a = s.items[1];
    expect(a).toMatchObject({ kind: "assistant", text: "Hello", streaming: true });
    const done = applyEvent(s, { type: "text", text: "Hello!", parentToolUseId: null, synthetic: false });
    expect(done.items[1]).toMatchObject({ kind: "assistant", text: "Hello!", streaming: false });
    expect(done.streamingId).toBeNull();
    expect(done.items).toHaveLength(2);
  });

  it("tool use, permission round trip, result with changed files", () => {
    let s = fold([
      { type: "userMessage", text: "write" },
      { type: "toolUse", id: "t1", name: "Write", input: { file_path: "C:\\p\\a.txt", content: "x" }, parentToolUseId: null },
      {
        type: "permissionRequest",
        requestId: "r1",
        toolName: "Write",
        toolUseId: "t1",
        input: { file_path: "C:\\p\\a.txt", content: "x" },
        description: null,
        blockedPath: null,
        risk: null,
        canRemember: true,
      },
    ]);
    expect(s.items.map((i) => i.kind)).toEqual(["user", "tool", "permission"]);
    expect(s.items[2]).toMatchObject({ status: "pending" });
    s = applyEvent(s, { type: "permissionResolved", requestId: "r1", decision: "allowOnce", byPolicy: false, message: null });
    expect(s.items[2]).toMatchObject({ status: "allowOnce" });
    s = applyEvent(s, { type: "toolResult", toolUseId: "t1", content: "ok", isError: false, parentToolUseId: null });
    s = applyEvent(s, { type: "fileChanged", path: "C:\\p\\a.txt" });
    s = applyEvent(s, { type: "fileChanged", path: "C:\\p\\a.txt" });
    s = applyEvent(s, { type: "result", isError: false, subtype: "success", result: "ok", durationMs: 5, numTurns: 1, totalCostUsd: 0, usage });
    expect(s.items[1]).toMatchObject({ kind: "tool", result: { content: "ok", isError: false } });
    expect(s.items[3]).toMatchObject({ kind: "result", files: ["C:\\p\\a.txt"] });
    expect(s.running).toBe(false);
  });

  it("nests sub-agent items in the Task card", () => {
    const s = fold([
      { type: "toolUse", id: "task", name: "Task", input: { description: "explore" }, parentToolUseId: null },
      { type: "toolUse", id: "g1", name: "Grep", input: { pattern: "x" }, parentToolUseId: "task" },
      { type: "text", text: "found", parentToolUseId: "task", synthetic: false },
      { type: "toolResult", toolUseId: "g1", content: "a.rs", isError: false, parentToolUseId: "task" },
    ]);
    expect(s.items).toHaveLength(1);
    const task = s.items[0];
    expect(task?.kind === "tool" && task.children.map((c) => c.kind)).toEqual(["tool", "assistant"]);
    expect(task?.kind === "tool" && task.children[0]).toMatchObject({ result: { content: "a.rs" } });
  });

  it("capabilities, auth errors, mode / model notices, exit settles pending permissions", () => {
    let s = fold([
      { type: "capabilities", models: [{ value: "opus", displayName: "Opus", description: null }], commands: [], account: { loggedIn: false, authMethod: null, subscriptionType: null, email: null } },
      { type: "init", claudeSessionId: "c1", model: "m", cwd: null, tools: [], permissionMode: "default" },
      { type: "modeChanged", mode: "default" },
      { type: "modeChanged", mode: "plan" },
      { type: "modelChanged", model: "haiku" },
      { type: "error", message: "Not logged in", authRequired: true },
    ]);
    expect(s.authRequired).toBe(true);
    expect(s.models[0]?.value).toBe("opus");
    expect(s.claudeSessionId).toBe("c1");
    expect(s.items.filter((i) => i.kind === "notice").map((i) => (i.kind === "notice" ? i.notice : ""))).toEqual(["modeChanged", "modelChanged"]);
    s = applyEvent(s, {
      type: "permissionRequest",
      requestId: "r",
      toolName: "Bash",
      toolUseId: null,
      input: {},
      description: null,
      blockedPath: null,
      risk: null,
      canRemember: true,
    });
    s = applyEvent(s, { type: "exited", code: 0 });
    expect(s.items.find((i) => i.kind === "permission")).toMatchObject({ status: "deny", byPolicy: true });
  });

  it("applyAll (resumed transcript) leaves the chat idle", () => {
    const s = applyAll(emptyChat(), [
      { type: "userMessage", text: "a" },
      { type: "textDelta", text: "b", parentToolUseId: null },
    ]);
    expect(s.running).toBe(false);
    expect(s.streamingId).toBeNull();
  });
});

describe("helpers", () => {
  it("relative paths (Windows case-insensitive, POSIX, outside root)", () => {
    expect(relativeTo("C:\\Work\\App", "c:\\work\\app\\src\\a.ts")).toBe("src/a.ts");
    expect(relativeTo("/home/me/app", "/home/me/app/b.rs")).toBe("b.rs");
    expect(relativeTo("/home/me/app", "/home/me/other/b.rs")).toBeNull();
    expect(relativeTo("C:\\Work\\App", "src\\x.ts", "C:\\Work\\App")).toBe("src/x.ts");
    expect(relativeTo(null, "/x")).toBeNull();
  });

  it("tool subjects, edit pairs and todos", () => {
    expect(toolSubject("Bash", { command: "ls" })).toBe("ls");
    expect(toolSubject("Grep", { pattern: "foo" })).toBe("foo");
    expect(editPairs("Edit", { old_string: "a", new_string: "b" })).toEqual([["a", "b"]]);
    expect(editPairs("MultiEdit", { edits: [{ old_string: "1", new_string: "2" }] })).toEqual([["1", "2"]]);
    expect(editPairs("Write", { content: "x" })).toEqual([["", "x"]]);
    expect(todos({ todos: [{ content: "A", status: "completed" }, { content: "B", status: "weird" }] })).toEqual([
      { content: "A", status: "completed" },
      { content: "B", status: "pending" },
    ]);
  });

  it("line diff", () => {
    expect(lineDiff("a\nb\nc", "a\nB\nc")).toEqual([
      { type: "same", text: "a" },
      { type: "del", text: "b" },
      { type: "add", text: "B" },
      { type: "same", text: "c" },
    ]);
  });

  it("composer popups, mentions and keys", () => {
    expect(composerPopup("/com", 4)).toEqual({ kind: "slash", query: "com" });
    expect(composerPopup("fix @src/ma", 11)).toEqual({ kind: "mention", query: "src/ma", start: 4 });
    expect(composerPopup("a@b", 3)).toBeNull();
    expect(composerPopup("hello", 5)).toBeNull();
    expect(insertMention("fix @sr now", 4, 7, "src/main.rs")).toEqual({ text: "fix @src/main.rs  now", caret: 17 });
    expect(composerKey({ key: "Enter", shiftKey: false })).toBe("send");
    expect(composerKey({ key: "Enter", shiftKey: true })).toBeNull();
    expect(composerKey({ key: "Enter", shiftKey: false, isComposing: true })).toBeNull();
    expect(composerKey({ key: "Escape", shiftKey: false })).toBe("interrupt");
  });

  it("selection prompt carries file:line", () => {
    const p = selectionPrompt({ path: "src/a.ts", startLine: 3, endLine: 5, text: "x()", language: "typescript" });
    expect(p.startsWith("@src/a.ts:3-5\n```typescript\nx()\n```")).toBe(true);
    expect(selectionPrompt({ path: "a", startLine: 2, endLine: 2, text: "y" })).toContain("@a:2\n");
  });
});
