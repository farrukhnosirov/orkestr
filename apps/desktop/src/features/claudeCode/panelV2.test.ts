// Claude Code panel v2 (pure parts): chat list grouping / search / pin /
// rename / hide, command catalogue filtering, plan usage tones, chat totals,
// Markdown export and image data URLs.
import type { ClaudeHistoryEntry, ClaudeSessionInfo, ClaudeSlashCommand } from "@orkestr/protocol";
import { describe, expect, it } from "vitest";

import { catalogEntries, catalogSections, commandText } from "./catalog";
import { buildChatList, chatGroup, folderName, groupChats, searchChats } from "./chatList";
import { chatMarkdown, chatTotals, exportFileName, lastAnswer, lastPrompt, msUntil, parseDataUrl, usageTone } from "./chatTools";
import { applyAll, emptyChat } from "./model";

const NOW = new Date(2026, 9, 8, 15, 0, 0); // 8 Oct 2026, 15:00 local
const iso = (d: Date) => d.toISOString();
const daysAgo = (n: number, h = 12) => iso(new Date(2026, 9, 8 - n, h, 0, 0));

function hist(id: string, updatedAt: string, patch: Partial<ClaudeHistoryEntry> = {}): ClaudeHistoryEntry {
  return { claudeSessionId: id, title: null, firstPrompt: `prompt ${id}`, createdAt: null, updatedAt, messages: 4, ...patch };
}

const A = "06a417de-ea3e-4314-9648-2072200e551a";
const B = "06a417de-ea3e-4314-9648-2072200e551b";
const C = "06a417de-ea3e-4314-9648-2072200e551c";
const D = "06a417de-ea3e-4314-9648-2072200e551d";

function live(sessionId: string, patch: Partial<ClaudeSessionInfo> = {}): ClaudeSessionInfo {
  return {
    sessionId,
    projectId: "p",
    cwd: "C:\\work\\app",
    claudeSessionId: null,
    model: null,
    permissionMode: "default",
    bypassAllowed: false,
    state: "idle",
    title: "live title",
    pid: 1,
    startedAt: daysAgo(0, 14),
    error: null,
    ...patch,
  };
}

describe("chat list", () => {
  const history = [hist(A, daysAgo(0, 10)), hist(B, daysAgo(1)), hist(C, daysAgo(4), { title: "CLI summary" }), hist(D, daysAgo(30))];

  it("groups by day: today / yesterday / this week / earlier, newest first", () => {
    const sections = groupChats(buildChatList([], history, {}), NOW);
    expect(sections.map((s) => [s.group, s.entries.map((e) => e.key)])).toEqual([
      ["today", [A]],
      ["yesterday", [B]],
      ["week", [C]],
      ["earlier", [D]],
    ]);
    expect(chatGroup("not a date", NOW)).toBe("earlier");
  });

  it("titles: custom label → CLI summary → first prompt; pinned go first; hidden are left out", () => {
    const meta = {
      [D]: { claudeSessionId: D, title: "My renamed chat", pinned: true, hidden: false },
      [B]: { claudeSessionId: B, title: null, pinned: false, hidden: true },
    };
    const list = buildChatList([], history, meta);
    const byKey = Object.fromEntries(list.map((e) => [e.key, e]));
    expect(byKey[D]?.title).toBe("My renamed chat");
    expect(byKey[D]?.customTitle).toBe(true);
    expect(byKey[C]?.title).toBe("CLI summary");
    expect(byKey[A]?.title).toBe(`prompt ${A}`);
    const sections = groupChats(list, NOW);
    expect(sections[0]).toMatchObject({ group: "pinned", entries: [{ key: D }] });
    expect(sections.flatMap((s) => s.entries.map((e) => e.key))).not.toContain(B);
    expect(groupChats(list, NOW, true).flatMap((s) => s.entries.map((e) => e.key))).toContain(B);
  });

  it("merges an open tab with its past conversation and marks it running", () => {
    const list = buildChatList(
      [
        { info: live("s1", { claudeSessionId: A }), claudeSessionId: A, running: true },
        { info: live("s2"), claudeSessionId: null, running: false },
      ],
      history,
      { [A]: { claudeSessionId: A, title: null, pinned: false, hidden: true } },
    );
    const a = list.find((e) => e.key === A);
    expect(a).toMatchObject({ sessionId: "s1", running: true, live: true, hidden: false, messages: 4, title: `prompt ${A}` });
    expect(list.find((e) => e.key === "live:s2")).toMatchObject({ claudeSessionId: null, title: "live title", live: true });
    // An ended tab is not "live".
    const ended = buildChatList([{ info: live("s3", { state: "exited" }), claudeSessionId: null, running: true }], [], {});
    expect(ended[0]).toMatchObject({ live: false, running: false });
  });

  it("search matches every word in title, folder or id (case-insensitive)", () => {
    const list = buildChatList([{ info: live("s1"), claudeSessionId: null, running: false }], history, {});
    expect(searchChats(list, "CLI summ").map((e) => e.key)).toEqual([C]);
    expect(searchChats(list, "work app").map((e) => e.key)).toEqual(["live:s1"]);
    expect(searchChats(list, "   ")).toHaveLength(list.length);
    expect(searchChats(list, "nothing-like-this")).toEqual([]);
    expect(folderName("C:\\work\\app\\")).toBe("app");
    expect(folderName(null)).toBeNull();
  });
});

describe("command catalogue", () => {
  const cmds: ClaudeSlashCommand[] = [
    { name: "compact", description: "Free up context", argumentHint: "<instructions>", builtin: true },
    { name: "clear", description: "Clear", argumentHint: null, builtin: true },
    { name: "usage", description: "Usage", argumentHint: null, builtin: true },
    { name: "security-review", description: "Security", argumentHint: null, builtin: true },
    { name: "design:ux-copy", description: "Write UX copy", argumentHint: "<context>", builtin: false },
    { name: "compact", description: "dup", argumentHint: null },
    { name: "__internal", description: "hidden", argumentHint: null },
  ];

  it("shows only the reported commands, deduplicated, with curated groups", () => {
    const entries = catalogEntries(cmds);
    expect(entries.map((e) => e.name)).toEqual(["compact", "clear", "usage", "security-review", "design:ux-copy"]);
    const by = Object.fromEntries(entries.map((e) => [e.name, e]));
    expect(by.compact).toMatchObject({ group: "context", curated: true, arg: true, argumentHint: "<instructions>" });
    expect(by.clear).toMatchObject({ destructive: true });
    expect(by.usage).toMatchObject({ group: "account", native: "usage" });
    expect(by["security-review"]).toMatchObject({ group: "code" });
    expect(by["design:ux-copy"]).toMatchObject({ group: "custom", curated: false, icon: "puzzle", arg: true });
  });

  it("filters by name, CLI and localized description; groups in a fixed order", () => {
    const entries = catalogEntries(cmds);
    const describe_ = (e: { name: string }) => (e.name === "clear" ? "Начать с чистого контекста" : "");
    const all = catalogSections(entries, "", describe_);
    expect(all.map((s) => s.group)).toEqual(["context", "code", "account", "custom"]);
    expect(catalogSections(entries, "/comp", describe_).flatMap((s) => s.entries.map((e) => e.name))).toEqual(["compact"]);
    expect(catalogSections(entries, "чистого", describe_).flatMap((s) => s.entries.map((e) => e.name))).toEqual(["clear"]);
    expect(catalogSections(entries, "ux copy", describe_).flatMap((s) => s.entries.map((e) => e.name))).toEqual(["design:ux-copy"]);
    expect(catalogSections(entries, "", describe_, "code").map((s) => s.group)).toEqual(["code"]);
  });

  it("builds the command text", () => {
    expect(commandText("compact", "  keep   the API notes ")).toBe("/compact keep the API notes");
    expect(commandText("clear", "")).toBe("/clear");
  });
});

describe("usage and chat tools", () => {
  it("usage tone follows percent and severity", () => {
    expect(usageTone({ percent: 10, severity: "normal" })).toBe("ok");
    expect(usageTone({ percent: 75, severity: null })).toBe("warn");
    expect(usageTone({ percent: 20, severity: "warning" })).toBe("warn");
    expect(usageTone({ percent: 95, severity: null })).toBe("danger");
    expect(usageTone(null)).toBe("ok");
    expect(msUntil("2026-10-08T10:40:00Z", Date.parse("2026-10-08T10:00:00Z"))).toBe(40 * 60_000);
    expect(msUntil("2026-10-08T10:40:00Z", Date.parse("2026-10-08T11:00:00Z"))).toBe(0);
    expect(msUntil(null, 0)).toBeNull();
  });

  const usage = { inputTokens: 10, outputTokens: 5, cacheReadTokens: 2, cacheWriteTokens: 1 };
  const chat = applyAll(emptyChat(), [
    { type: "userMessage", text: "first question" },
    { type: "text", text: "first answer", parentToolUseId: null, synthetic: false },
    { type: "toolUse", id: "t1", name: "Read", input: { file_path: "a.ts" }, parentToolUseId: null },
    { type: "result", isError: false, subtype: "success", result: null, durationMs: 1000, numTurns: 1, totalCostUsd: 0.5, usage },
    { type: "userMessage", text: "/compact" },
    { type: "text", text: "Compacted", parentToolUseId: null, synthetic: true },
    { type: "result", isError: false, subtype: "success", result: null, durationMs: 500, numTurns: 1, totalCostUsd: null, usage },
  ]);

  it("chat totals, last answer and last prompt", () => {
    expect(chatTotals(chat)).toEqual({ turns: 2, inputTokens: 26, outputTokens: 10, costUsd: 0.5, durationMs: 1500 });
    expect(lastAnswer(chat)).toBe("first answer");
    expect(lastPrompt(chat)).toBe("first question");
    expect(lastAnswer(emptyChat())).toBeNull();
  });

  it("exports Markdown", () => {
    const md = chatMarkdown(chat, "My chat", { user: "Вы", claude: "Claude", tool: "Инструмент" });
    expect(md).toContain("# My chat");
    expect(md).toContain("## Вы\n\nfirst question");
    expect(md).toContain("## Claude\n\nfirst answer");
    expect(md).toContain("> Инструмент: `Read`");
    expect(exportFileName("Исправь баг: API!", new Date("2026-10-08T00:00:00Z"))).toBe("claude-исправь-баг-api-2026-10-08.md");
  });

  it("parses image data URLs", () => {
    expect(parseDataUrl("data:image/png;base64,iVBOR=")).toEqual({ mediaType: "image/png", data: "iVBOR=" });
    expect(parseDataUrl("data:text/html;base64,PGI+")).toBeNull();
    expect(parseDataUrl("nope")).toBeNull();
  });
});
