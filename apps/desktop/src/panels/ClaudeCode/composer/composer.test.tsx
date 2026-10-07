// Claude Code composer: keys, @-mentions (files.find), slash commands,
// queueing, disabled reasons, selection attachment, model picker, Russian UI.
import type { ClaudeSlashCommand } from "@orkestr/protocol";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const calls: { method: string; params: unknown }[] = [];
let matches: string[] = [];
vi.mock("../../../core/client", () => ({
  getClient: () => ({
    call: (method: string, params: unknown) => {
      calls.push({ method, params });
      return Promise.resolve({ matches: matches.map((path) => ({ path, score: 1 })), filesScanned: 10, truncated: false });
    },
  }),
  maybeClient: () => null,
}));
vi.mock("../../../features/claudeCode/bridge", () => ({ currentSelection: () => null }));

import { i18n } from "../../../i18n";
import { Composer, type ComposerViewProps, mentionedFiles, ModelPicker, slashSuggestions } from "./index";

function setup(over: Partial<ComposerViewProps> = {}) {
  const onSend = vi.fn<ComposerViewProps["onSend"]>(() => Promise.resolve(true));
  const onInterrupt = vi.fn();
  const onDequeue = vi.fn();
  const props: ComposerViewProps = {
    projectId: "p1",
    disabled: false,
    running: false,
    queue: [],
    commands: [],
    insert: null,
    onInsertConsumed: () => {},
    onSend,
    onInterrupt,
    onDequeue,
    ...over,
  };
  const view = render(<Composer {...props} />);
  const input = screen.getByRole("combobox") as HTMLTextAreaElement;
  const type = (value: string) => fireEvent.change(input, { target: { value } });
  return { ...view, props, input, type, onSend, onInterrupt, onDequeue };
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
  calls.length = 0;
  matches = [];
});
afterEach(cleanup);

describe("Composer keys", () => {
  it("Enter sends and clears, Shift+Enter does not send", async () => {
    const { input, type, onSend } = setup();
    type("hello");
    fireEvent.keyDown(input, { key: "Enter", shiftKey: true });
    expect(onSend).not.toHaveBeenCalled();
    await act(async () => {
      fireEvent.keyDown(input, { key: "Enter" });
    });
    expect(onSend).toHaveBeenCalledWith({ text: "hello", attachments: [] });
    expect(input.value).toBe("");
  });

  it("keeps the text when the parent refuses the message", async () => {
    const { input, type, onSend } = setup();
    onSend.mockResolvedValueOnce(false);
    type("keep me");
    await act(async () => {
      fireEvent.keyDown(input, { key: "Enter" });
    });
    expect(input.value).toBe("keep me");
  });

  it("does not send whitespace", () => {
    const { input, type, onSend } = setup();
    type("   ");
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onSend).not.toHaveBeenCalled();
  });

  it("Esc interrupts only while running; the Stop button too", () => {
    const idle = setup();
    fireEvent.keyDown(idle.input, { key: "Escape" });
    expect(idle.onInterrupt).not.toHaveBeenCalled();
    cleanup();
    const busy = setup({ running: true });
    fireEvent.keyDown(busy.input, { key: "Escape" });
    expect(busy.onInterrupt).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("button", { name: "Stop" }));
    expect(busy.onInterrupt).toHaveBeenCalledTimes(2);
  });
});

describe("@-mentions", () => {
  it("searches files.find, navigates with arrows, inserts @path and returns it as an attachment", async () => {
    matches = ["src/app.tsx", "src/apple.ts"];
    const { input, type, onSend } = setup();
    type("look at @app");
    expect(await screen.findByRole("option", { name: "src/app.tsx" })).toBeTruthy();
    expect(calls).toContainEqual({ method: "files.find", params: { projectId: "p1", query: "app", maxResults: 8 } });
    expect(screen.getByRole("option", { name: "src/app.tsx" }).getAttribute("aria-selected")).toBe("true");
    fireEvent.keyDown(input, { key: "ArrowDown" });
    expect(screen.getByRole("option", { name: "src/apple.ts" }).getAttribute("aria-selected")).toBe("true");
    fireEvent.keyDown(input, { key: "ArrowUp" });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(input.value).toBe("look at @src/app.tsx ");
    expect(onSend).not.toHaveBeenCalled();
    expect(screen.queryByRole("listbox")).toBeNull();

    type("look at @src/app.tsx please");
    await act(async () => {
      fireEvent.keyDown(input, { key: "Enter" });
    });
    expect(onSend).toHaveBeenCalledWith({
      text: "look at @src/app.tsx please",
      attachments: [{ kind: "file", path: "src/app.tsx" }],
    });
  });

  it("Esc closes the popup instead of interrupting", async () => {
    matches = ["a.ts"];
    const { input, type, onInterrupt } = setup({ running: true });
    type("@a");
    await screen.findByRole("option", { name: "a.ts" });
    fireEvent.keyDown(input, { key: "Escape" });
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(onInterrupt).not.toHaveBeenCalled();
  });

  it("drops a picked file whose mention was deleted", () => {
    expect(mentionedFiles("see @a.ts and @b c.ts", ["a.ts", "b c.ts"])).toEqual(["a.ts"]);
    expect(mentionedFiles('see @"b c.ts"', ["b c.ts"])).toEqual(["b c.ts"]);
    expect(mentionedFiles("mail me@a.ts", ["a.ts"])).toEqual([]);
  });
});

describe("slash commands", () => {
  const cli: ClaudeSlashCommand[] = [
    { name: "deploy", description: "Ship it", argumentHint: null },
    { name: "compact", description: "cli text", argumentHint: "[instructions]" },
  ];

  it("lists built-ins and CLI commands, deduplicated", () => {
    expect(slashSuggestions(cli, "").map((s) => s.name)).toEqual(["compact", "clear", "init", "cost", "review", "model", "deploy"]);
    expect(slashSuggestions(cli, "co").map((s) => s.name)).toEqual(["compact", "cost"]);
  });

  it("shows the popup and completes the command", () => {
    const { input, type } = setup({ commands: cli });
    type("/de");
    const option = screen.getByRole("option", { name: /\/deploy/ });
    expect(option.textContent).toContain("Ship it");
    type("/com");
    expect(screen.getByRole("option", { name: /\/compact \[instructions\]/ }).textContent).toContain(
      "Compact the conversation to free up context",
    );
    fireEvent.keyDown(input, { key: "Tab" });
    expect(input.value).toBe("/compact ");
    expect(screen.queryByRole("listbox")).toBeNull();
  });
});

describe("queue and disabled state", () => {
  it("queues one message while running", async () => {
    const { input, type, onSend } = setup({ running: true });
    expect(screen.getByRole("button", { name: "Queue" })).toBeTruthy();
    type("next");
    await act(async () => {
      fireEvent.keyDown(input, { key: "Enter" });
    });
    expect(onSend).toHaveBeenCalledTimes(1);
  });

  it("shows the queued message and blocks a second one", () => {
    const { input, type, onSend, onDequeue } = setup({ running: true, queue: ["first one"] });
    const queued = screen.getByRole("list", { name: "Queued" });
    expect(queued.textContent).toContain("first one");
    type("second");
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onSend).not.toHaveBeenCalled();
    expect(screen.getByRole("status").textContent).toContain("already queued");
    fireEvent.click(screen.getByRole("button", { name: "Remove from the queue" }));
    expect(onDequeue).toHaveBeenCalledWith(0);
  });

  it("explains why it is disabled", () => {
    const { input } = setup({ disabled: true, disabledReason: "notSignedIn" });
    expect(input.disabled).toBe(true);
    expect(screen.getByRole("status").textContent).toContain("Sign in to Claude Code");
    cleanup();
    setup({ disabled: true, disabledReason: "noBinary" });
    expect(screen.getByRole("status").textContent).toContain("CLI was not found");
  });
});

describe("selection attachment", () => {
  it("attaches the editor selection as a chip and sends it with @path:lines", async () => {
    const selection = { path: "src/a.ts", startLine: 3, endLine: 4, text: "const a = 1;\nconst b = 2;", language: "ts" };
    const { input, type, onSend } = setup({ selection });
    fireEvent.click(screen.getByRole("button", { name: /Attach selection/ }));
    expect(screen.getByText("Selection: src/a.ts:3-4")).toBeTruthy();
    type("explain");
    await act(async () => {
      fireEvent.keyDown(input, { key: "Enter" });
    });
    const payload = onSend.mock.calls[0]?.[0];
    expect(payload?.text).toBe("@src/a.ts:3-4\n```ts\nconst a = 1;\nconst b = 2;\n```\nexplain");
    expect(payload?.attachments).toEqual([{ kind: "selection", ...selection }]);
    expect(screen.queryByText("Selection: src/a.ts:3-4")).toBeNull();
  });
});

describe("ModelPicker", () => {
  it("switches aliases and accepts a custom id", () => {
    const onChange = vi.fn();
    render(
      <ModelPicker
        value="default"
        models={[{ value: "claude-opus-4-1", displayName: "Opus 4.1", description: null }]}
        onChange={onChange}
      />,
    );
    const select = screen.getByRole("combobox", { name: "Model" });
    fireEvent.change(select, { target: { value: "opus" } });
    expect(onChange).toHaveBeenLastCalledWith("opus");
    expect(screen.getByRole("option", { name: "Opus 4.1" })).toBeTruthy();
    fireEvent.change(select, { target: { value: "__custom__" } });
    const id = screen.getByRole("textbox", { name: "Model ID" });
    fireEvent.change(id, { target: { value: "claude-x-1" } });
    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    expect(onChange).toHaveBeenLastCalledWith("claude-x-1");
  });
});

describe("Russian", () => {
  it("renders the composer in Russian", async () => {
    await i18n.changeLanguage("ru");
    setup({ running: true, queue: ["первое"] });
    expect(screen.getByRole("combobox").getAttribute("placeholder")).toBe("Claude работает — сообщение встанет в очередь");
    expect(screen.getByRole("button", { name: "Остановить" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "В очередь" })).toBeTruthy();
    expect(screen.getByRole("list", { name: "В очереди" })).toBeTruthy();
    cleanup();
    const { type } = setup();
    type("/in");
    expect(screen.getByRole("option", { name: /\/init/ }).textContent).toContain("Создать CLAUDE.md");
    await waitFor(() => expect(screen.getByRole("status").textContent).toContain("Shift+Enter — новая строка"));
  });
});
