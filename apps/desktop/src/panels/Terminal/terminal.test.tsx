import type { TerminalSession } from "@orkestr/protocol";
import { act, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { commands, matchesShortcut } from "../../features/commands/registry";
import { applyLanguagePreference } from "../../i18n";
import { getPanel } from "../registry";
import TerminalPanel from "./index";
import {
  dedupeChunk,
  isReadOnly,
  nextActiveAfterClose,
  normalizeTitle,
  orderShells,
  sessionSubtitle,
  sessionsForProject,
  themeFromTokens,
  utf8Length,
} from "./logic";
import { scopeKey, useTerminalUi } from "./store";

const P = "0190f0b0-0000-7000-8000-000000000001";

function session(patch: Partial<TerminalSession> = {}): TerminalSession {
  return {
    terminalId: "0190f0b0-0000-7000-8000-0000000000t1",
    projectId: P,
    owner: { kind: "user" },
    kind: "pty",
    shellId: "pwsh",
    shellKind: "PowerShell7",
    title: "PowerShell 7",
    cwd: "C:/work",
    cols: 120,
    rows: 30,
    pid: 42,
    status: "running",
    exitCode: null,
    readOnly: false,
    createdAt: "2026-10-02T00:00:00Z",
    outputBytes: 0,
    ...patch,
  };
}

describe("dedupeChunk", () => {
  it("writes chunks after the replayed history", () => {
    expect(dedupeChunk(10, 10, "abc")).toEqual({ text: "abc", written: 13 });
  });

  it("drops chunks already contained in the history", () => {
    expect(dedupeChunk(10, 4, "abcdef")).toEqual({ text: "", written: 10 });
  });

  it("keeps only the new suffix of overlapping chunks (UTF-8 byte offsets)", () => {
    // "é" is 2 bytes: history ended after "aé" (3 bytes) of "aébc".
    expect(dedupeChunk(3, 0, "aébc")).toEqual({ text: "bc", written: 5 });
    expect(utf8Length("€😀")).toBe(7);
  });

  it("always writes chunks without offsets", () => {
    expect(dedupeChunk(10, undefined, "zz")).toEqual({ text: "zz", written: 12 });
  });
});

describe("session helpers", () => {
  it("treats agent sessions and command views as read-only", () => {
    expect(isReadOnly(session())).toBe(false);
    expect(isReadOnly(session({ owner: { kind: "agent", agentId: P } }))).toBe(true);
    expect(isReadOnly(session({ kind: "commandView" }))).toBe(true);
  });

  it("filters sessions by project and keeps project-less ones", () => {
    const a = session();
    const b = session({ terminalId: "b", projectId: null });
    const c = session({ terminalId: "c", projectId: "other" });
    expect(sessionsForProject([a, b, c], P).map((s) => s.terminalId)).toEqual([a.terminalId, "b"]);
  });

  it("describes status", () => {
    expect(sessionSubtitle(session())).toBe("pwsh · pid 42");
    expect(sessionSubtitle(session({ status: "exited", exitCode: 3 }))).toBe("exited with code 3");
  });

  it("validates titles like the core", () => {
    expect(normalizeTitle("  Build ")).toBe("Build");
    expect(normalizeTitle("   ")).toBeNull();
    expect(normalizeTitle("x".repeat(81))).toBeNull();
    expect(normalizeTitle("a\u0007b")).toBeNull();
  });

  it("picks the neighbour tab after closing", () => {
    expect(nextActiveAfterClose(["a", "b", "c"], "b")).toBe("c");
    expect(nextActiveAfterClose(["a", "b", "c"], "c")).toBe("b");
    expect(nextActiveAfterClose(["a"], "a")).toBeNull();
  });

  it("orders the default shell first", () => {
    const shells = orderShells([
      { id: "cmd", kind: "Cmd", name: "Command Prompt", path: "cmd.exe", args: [], isDefault: false },
      { id: "pwsh", kind: "PowerShell7", name: "PowerShell 7", path: "pwsh.exe", args: [], isDefault: true },
    ]);
    expect(shells[0]?.id).toBe("pwsh");
  });

  it("derives the xterm theme from tokens with fallbacks", () => {
    document.documentElement.style.setProperty("--ok-color-accent", "#123456");
    const theme = themeFromTokens();
    expect(theme.cursor).toBe("#123456");
    expect(theme.foreground).toBeTruthy();
    document.documentElement.style.removeProperty("--ok-color-accent");
  });
});

describe("terminal wiring", () => {
  it("is available now in the panel registry", () => {
    expect(getPanel("terminal")?.phase).toBe(0);
  });

  it("registers Open Terminal on Ctrl+`", () => {
    const cmd = commands.find((c) => c.id === "terminal.open");
    expect(cmd?.shortcut).toBe("Ctrl+`");
    const base = { ctrlKey: true, metaKey: false, shiftKey: false, altKey: false };
    expect(matchesShortcut({ ...base, key: "`", code: "Backquote" }, "Ctrl+`")).toBe(true);
    // Non-US layouts report another key but the same physical code.
    expect(matchesShortcut({ ...base, key: "ё", code: "Backquote" }, "Ctrl+`")).toBe(true);
    expect(matchesShortcut({ ...base, shiftKey: true, key: "`", code: "Backquote" }, "Ctrl+`")).toBe(false);
    expect(matchesShortcut({ ...base, key: "k", code: "KeyK" }, "Ctrl+K")).toBe(true);
  });

  it("the command requests a terminal", () => {
    const before = useTerminalUi.getState().openRequest;
    void commands.find((c) => c.id === "terminal.open")?.run({ projectId: null });
    expect(useTerminalUi.getState().openRequest).toBe(before + 1);
  });

  it("remembers the active tab per project", () => {
    useTerminalUi.getState().setActive(P, "t1");
    expect(useTerminalUi.getState().active[scopeKey(P)]).toBe("t1");
    useTerminalUi.getState().setActive(P, null);
    expect(useTerminalUi.getState().active[scopeKey(P)]).toBeUndefined();
  });

  it("shows a waiting state without a core connection", () => {
    render(<TerminalPanel projectId={null} />);
    expect(screen.getByText("Waiting for the core")).toBeTruthy();
  });

  it("speaks Russian after a language switch", async () => {
    await act(async () => {
      await applyLanguagePreference("ru");
    });
    try {
      render(<TerminalPanel projectId={null} />);
      expect(screen.getAllByText("Ожидание ядра").length).toBeGreaterThan(0);
      expect(commands.find((c) => c.id === "terminal.open")?.title).toBe("Открыть терминал");
      expect(sessionSubtitle(session({ status: "exited", exitCode: 3 }))).toBe("завершён с кодом 3");
      expect(sessionSubtitle(session())).toBe("pwsh · PID 42");
    } finally {
      await act(async () => {
        await applyLanguagePreference("en");
      });
    }
  });
});
