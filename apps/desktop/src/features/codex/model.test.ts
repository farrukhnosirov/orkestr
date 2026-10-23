// Codex chat model: event folding (deltas + final text, tool pairing,
// approvals, turns, errors), the chat list, slash commands and helpers.
import type { CodexEvent, CodexLimits, CodexSessionInfo, CodexThread, JsonValue } from "@orkestr/protocol";
import { describe, expect, it } from "vitest";

import { buildCodexChatList, groupCodexChats, searchCodexChats } from "./chatList";
import { parseCodexSlash } from "./commands";
import {
  codexErrorKey,
  applyCodexAll,
  applyCodexEvent,
  emptyCodexChat,
  limitTone,
  maxLimitPercent,
  patchFiles,
  patchLines,
  pendingApprovals,
  settleCodexChat,
  windowKind,
  type CodexChat,
} from "./model";

function fold(events: CodexEvent[], from: CodexChat = emptyCodexChat()): CodexChat {
  return events.reduce(applyCodexEvent, from);
}

const usage = { inputTokens: 1200, cachedInputTokens: 200, outputTokens: 300, reasoningTokens: 50 };

describe("applyCodexEvent", () => {
  it("streams deltas per item and replaces them with the complete text", () => {
    const s = fold([
      { type: "threadStarted", threadId: "th1", model: "gpt-5-codex", effort: "medium" },
      { type: "turnStarted", turnId: "t1" },
      { type: "userMessage", text: "hi" },
      { type: "textDelta", itemId: "m1", text: "Hel" },
      { type: "textDelta", itemId: "m1", text: "lo" },
    ]);
    expect(s.threadId).toBe("th1");
    expect(s.model).toBe("gpt-5-codex");
    expect(s.running).toBe(true);
    expect(s.items[1]).toMatchObject({ kind: "assistant", text: "Hello", streaming: true });
    const done = applyCodexEvent(s, { type: "text", itemId: "m1", text: "Hello!" });
    expect(done.items).toHaveLength(2);
    expect(done.items[1]).toMatchObject({ kind: "assistant", text: "Hello!", streaming: false });
    // A complete text without deltas still shows up; a second item is separate.
    const two = fold([{ type: "text", itemId: "m2", text: "Second" }], done);
    expect(two.items).toHaveLength(3);
    expect(two.items[2]).toMatchObject({ kind: "assistant", text: "Second", streaming: false });
  });

  it("pairs tool results with their tool use", () => {
    const s = fold([
      { type: "userMessage", text: "run" },
      { type: "toolUse", id: "c1", name: "Bash", input: { command: "npm test", cwd: "C:\\p" } },
      { type: "toolUse", id: "c2", name: "WebSearch", input: { query: "vitest" } },
      { type: "toolResult", toolUseId: "c1", content: "ok", isError: false },
      { type: "toolResult", toolUseId: "c2", content: "boom", isError: true },
      { type: "toolResult", toolUseId: "missing", content: "?", isError: false },
    ]);
    const tools = s.items.filter((it) => it.kind === "tool");
    expect(tools).toHaveLength(2);
    expect(tools[0]).toMatchObject({ id: "c1", name: "Bash", result: { content: "ok", isError: false } });
    expect(tools[1]).toMatchObject({ id: "c2", result: { content: "boom", isError: true } });
  });

  it("closes a streamed answer when a tool starts", () => {
    const s = fold([
      { type: "textDelta", itemId: "m1", text: "Let me look" },
      { type: "toolUse", id: "c1", name: "Read", input: { file_path: "a.ts" } },
    ]);
    expect(s.items[0]).toMatchObject({ kind: "assistant", streaming: false });
  });

  it("approval request → resolved, with policy decisions", () => {
    let s = fold([
      { type: "userMessage", text: "delete" },
      {
        type: "approvalRequest",
        requestId: "r1",
        kind: "command",
        toolName: "Bash",
        input: { command: "rm -rf build", cwd: "C:\\p" },
        reason: "clean the build",
        risk: { level: "dangerous", category: "DeleteFiles", reason: "deletes files" } as never,
        canRemember: false,
      },
    ]);
    expect(pendingApprovals(s)).toHaveLength(1);
    expect(s.items[1]).toMatchObject({ kind: "approval", status: "pending", approval: "command", reason: "clean the build" });
    // The same request twice is shown once.
    s = applyCodexEvent(s, { type: "approvalRequest", requestId: "r1", kind: "command", toolName: "Bash", input: {}, reason: null, risk: null, canRemember: true });
    expect(s.items.filter((it) => it.kind === "approval")).toHaveLength(1);
    s = applyCodexEvent(s, { type: "approvalResolved", requestId: "r1", decision: "deny", byPolicy: true, message: "forbidden" });
    expect(s.items[1]).toMatchObject({ status: "deny", byPolicy: true, message: "forbidden" });
    expect(pendingApprovals(s)).toHaveLength(0);
  });

  it("turnCompleted ends the turn with status, duration and changed files; pending approvals are settled", () => {
    const s = fold([
      { type: "turnStarted", turnId: "t1" },
      { type: "userMessage", text: "edit" },
      { type: "textDelta", itemId: "m1", text: "Working" },
      { type: "fileChanged", path: "C:\\p\\a.ts" },
      { type: "fileChanged", path: "C:\\p\\a.ts" },
      { type: "approvalRequest", requestId: "r2", kind: "fileChange", toolName: "Edit", input: { file_path: "a.ts", changes: [] }, reason: null, risk: null, canRemember: true },
      { type: "tokenUsage", last: usage, total: usage, contextWindow: 200000 },
      { type: "turnCompleted", turnId: "t1", status: "interrupted", error: null, durationMs: 4200 },
    ]);
    expect(s.running).toBe(false);
    expect(s.turnFiles).toEqual([]);
    expect(s.tokens?.contextWindow).toBe(200000);
    const turn = s.items[s.items.length - 1];
    expect(turn).toMatchObject({ kind: "turn", status: "interrupted", durationMs: 4200, files: ["C:\\p\\a.ts"] });
    expect(s.items.find((it) => it.kind === "approval")).toMatchObject({ status: "abort", byPolicy: true });
    expect(s.items.find((it) => it.kind === "assistant")).toMatchObject({ streaming: false });
  });

  it("errors flag a needed sign-in and a reached plan limit; a completed turn clears the limit flag", () => {
    let s = fold([
      { type: "error", message: "401", authRequired: true, usageLimit: false, willRetry: false },
      { type: "error", message: "usage limit", authRequired: false, usageLimit: true, willRetry: false },
      { type: "error", message: "stream reset", authRequired: false, usageLimit: false, willRetry: true },
    ]);
    expect(s.authRequired).toBe(true);
    expect(s.usageLimit).toBe(true);
    expect(s.items.map((it) => it.kind)).toEqual(["error", "error", "error"]);
    expect(s.items[2]).toMatchObject({ willRetry: true });
    s = applyCodexEvent(s, { type: "turnCompleted", turnId: "t", status: "completed", error: null, durationMs: null });
    expect(s.usageLimit).toBe(false);
  });

  it("keeps reasoning summaries and one plan per turn; notes, model changes and exit", () => {
    let s = fold([
      { type: "userMessage", text: "plan it" },
      { type: "reasoning", text: "Look at the tests" },
      { type: "reasoning", text: "Then fix" },
      { type: "plan", text: "1. a" },
      { type: "plan", text: "1. a\n2. b" },
      { type: "modelChanged", model: "gpt-5", effort: "high" },
      { type: "info", text: "Context compacted" },
    ]);
    expect(s.items.filter((it) => it.kind === "reasoning")).toHaveLength(1);
    expect(s.items.find((it) => it.kind === "reasoning")).toMatchObject({ text: "Look at the tests\n\nThen fix" });
    expect(s.items.filter((it) => it.kind === "plan")).toEqual([expect.objectContaining({ text: "1. a\n2. b" })]);
    expect(s.model).toBe("gpt-5");
    expect(s.effort).toBe("high");
    s = fold([{ type: "userMessage", text: "again" }, { type: "plan", text: "new" }], s);
    expect(s.items.filter((it) => it.kind === "plan")).toHaveLength(2);
    s = applyCodexEvent(s, { type: "exited", code: 1 });
    expect(s.running).toBe(false);
    expect(s.items[s.items.length - 1]).toMatchObject({ kind: "notice", notice: "exited", text: "1" });
  });

  it("rate limits are kept; transcripts and settle never stay running", () => {
    const limits: CodexLimits = {
      available: true,
      plan: "plus",
      primary: { percent: 85, windowMinutes: 300, resetsAt: null },
      secondary: { percent: 20, windowMinutes: 10080, resetsAt: null },
      credits: null,
      limitReached: null,
      usageAllowed: true,
      fetchedAt: "2026-10-23T00:00:00Z",
      error: null,
    };
    const s = fold([{ type: "rateLimits", limits }, { type: "textDelta", itemId: "x", text: "a" }]);
    expect(s.limits?.plan).toBe("plus");
    expect(settleCodexChat(s)).toMatchObject({ running: false });
    const t = applyCodexAll(emptyCodexChat(), [{ type: "userMessage", text: "q" }, { type: "textDelta", itemId: "m", text: "a" }]);
    expect(t.running).toBe(false);
    expect(t.items[1]).toMatchObject({ streaming: false });
    expect(maxLimitPercent(limits)).toBe(85);
    expect(maxLimitPercent({ ...limits, available: false })).toBeNull();
    expect(limitTone(85)).toBe("warn");
    expect(limitTone(95)).toBe("danger");
    expect(limitTone(null)).toBe("ok");
    expect(windowKind(limits.primary, "primary")).toBe("hours");
    expect(windowKind(limits.secondary, "secondary")).toBe("week");
    expect(windowKind(null, "secondary")).toBe("secondary");
  });
});

describe("patch helpers", () => {
  it("reads Codex Edit inputs and classifies unified diff lines", () => {
    const input = {
      file_path: "C:\\p\\a.ts",
      changes: [
        { path: "C:\\p\\a.ts", kind: "update", diff: "--- a/a.ts\n+++ b/a.ts\n@@ -1,2 +1,2 @@\n keep\n-old\n+new\n" },
        { path: "C:\\p\\b.ts", kind: { add: {} } },
        { nope: true },
      ],
    } as unknown as JsonValue;
    const files = patchFiles(input);
    expect(files).toEqual([
      { path: "C:\\p\\a.ts", kind: "update", diff: expect.any(String) },
      { path: "C:\\p\\b.ts", kind: "add", diff: null },
    ]);
    expect(patchLines(files[0]?.diff ?? "")).toEqual([
      { type: "hunk", text: "@@ -1,2 +1,2 @@" },
      { type: "same", text: "keep" },
      { type: "del", text: "old" },
      { type: "add", text: "new" },
    ]);
    expect(patchFiles({ file_path: "x", old_string: "a", new_string: "b" })).toEqual([]);
  });
});

function session(patch: Partial<CodexSessionInfo> = {}): CodexSessionInfo {
  return {
    sessionId: "s1",
    projectId: "p",
    cwd: "C:\\work\\app",
    threadId: null,
    model: "gpt-5-codex",
    effort: null,
    mode: "ask",
    state: "idle",
    title: null,
    startedAt: "2026-10-23T09:00:00Z",
    error: null,
    ...patch,
  };
}

function thread(id: string, updatedAt: string, patch: Partial<CodexThread> = {}): CodexThread {
  return { id, title: `Chat ${id}`, cwd: "C:\\work\\app", model: "gpt-5-codex", createdAt: updatedAt, updatedAt, forkedFrom: null, ...patch };
}

describe("codex chat list", () => {
  const now = new Date("2026-10-23T12:00:00");

  it("merges saved threads and live sessions; live sessions come first", () => {
    const threads = [thread("a", "2026-10-23T08:00:00"), thread("b", "2026-10-20T08:00:00", { forkedFrom: "a" }), thread("c", "2026-09-01T08:00:00", { title: "Deploy pipeline" })];
    const live = [
      { info: session({ threadId: "a", state: "running", title: "Live A" }), threadId: "a", running: true },
      { info: session({ sessionId: "s2" }), threadId: null, running: false },
    ];
    const list = buildCodexChatList(live, threads);
    expect(list).toHaveLength(4);
    const a = list.find((e) => e.key === "a");
    expect(a).toMatchObject({ sessionId: "s1", live: true, running: true, title: "Live A" });
    expect(list.find((e) => e.key === "live:s2")).toMatchObject({ threadId: null, live: true });
    const groups = groupCodexChats(list, now);
    expect(groups.map((g) => g.group)).toEqual(["live", "week", "earlier"]);
    expect(groups[1]?.entries[0]).toMatchObject({ key: "b", forkedFrom: "a" });
    expect(searchCodexChats(list, "deploy").map((e) => e.key)).toEqual(["c"]);
  });

  it("an ended session is not live and keeps its thread resumable", () => {
    const list = buildCodexChatList([{ info: session({ threadId: "a", state: "exited" }), threadId: "a", running: true }], [thread("a", "2026-10-23T08:00:00")]);
    expect(list[0]).toMatchObject({ live: false, running: false, threadId: "a", sessionId: "s1" });
  });
});

describe("slash commands", () => {
  it("parses the panel's commands and ignores the rest", () => {
    expect(parseCodexSlash("/new")).toEqual({ command: "new", arg: "" });
    expect(parseCodexSlash("  /model  gpt-5 ")).toEqual({ command: "model", arg: "gpt-5" });
    expect(parseCodexSlash("/STATUS")).toEqual({ command: "status", arg: "" });
    expect(parseCodexSlash("/review")).toBeNull();
    expect(parseCodexSlash("fix /new things")).toBeNull();
  });
});

describe("codexErrorKey", () => {
  it("maps app-server failures to a localized message", () => {
    expect(codexErrorKey("server error: the Codex app-server ended")).toBe("errors.appServer");
    expect(codexErrorKey("the Codex app-server is not running")).toBe("errors.appServer");
    expect(codexErrorKey("network down")).toBeNull();
  });
});
