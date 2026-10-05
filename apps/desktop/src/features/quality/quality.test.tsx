import type { CheckDef, CheckResult, Event, QualityRun } from "@orkestr/protocol";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { i18n } from "../../i18n";
import { useEditorStore } from "../../store/editor";
import { commands } from "../commands/registry";
import { settingsSections } from "../settings/sections";
import {
  blankCheck,
  checkTone,
  latestStatus,
  locationOf,
  resultReason,
  runIdOfBuildId,
  runTotals,
  TEST_KINDS,
  validateProfile,
  workspaceRelative,
} from "./logic";
import { RunResults } from "./RunResults";
import { mergeRun, runIdOfEvent } from "./store";

const PROJECT = "0190f0b0-0000-7000-8000-000000000001";

function result(patch: Partial<CheckResult> = {}): CheckResult {
  return {
    checkId: "cargo-test",
    kind: "Test",
    toolchain: "Cargo",
    label: "cargo test",
    command: "cargo test --workspace",
    cwd: ".",
    status: "Passed",
    exitCode: 0,
    durationMs: 1200,
    passed: 3,
    failed: 0,
    skipped: 0,
    failures: [],
    diagnostics: [],
    rawTail: "",
    summary: "3 passed, 0 failed, 0 skipped",
    approvalId: null,
    startedAt: "2026-10-05T10:00:00Z",
    ...patch,
  };
}

function run(id: string, startedAt: string, results: CheckResult[], patch: Partial<QualityRun> = {}): QualityRun {
  return {
    id,
    projectId: PROJECT,
    taskId: null,
    agentId: null,
    target: { type: "project" },
    workspace: "C:/work/demo",
    kinds: [],
    status: "Passed",
    results,
    startedAt,
    finishedAt: startedAt,
    durationMs: 1500,
    ...patch,
  };
}

afterEach(async () => {
  cleanup();
  await act(async () => {
    await i18n.changeLanguage("en");
  });
});

describe("quality logic", () => {
  it("maps statuses to tones", () => {
    expect(checkTone("Passed")).toBe("success");
    expect(checkTone("TimedOut")).toBe("danger");
    expect(checkTone("Blocked")).toBe("warning");
    expect(checkTone("Skipped")).toBe("neutral");
  });

  it("resolves workspace-relative paths for jump-to-file", () => {
    expect(workspaceRelative("src/lib.rs", ".", "C:/work/demo")).toBe("src/lib.rs");
    expect(workspaceRelative("src/App.tsx", "apps/web", "C:/work/demo")).toBe("apps/web/src/App.tsx");
    expect(workspaceRelative(".\\src\\a.ts", "pkg", "C:/work/demo")).toBe("pkg/src/a.ts");
    expect(workspaceRelative("C:\\work\\demo\\crates\\x\\src\\lib.rs", ".", "C:\\work\\demo")).toBe("crates/x/src/lib.rs");
    expect(workspaceRelative("c:/WORK/demo/a.rs", ".", "C:/work/demo")).toBe("a.rs");
    expect(workspaceRelative("/home/u/proj/a.py", ".", "/home/u/proj")).toBe("a.py");
    expect(workspaceRelative("/home/u/other/a.py", ".", "/home/u/proj")).toBeNull();
    expect(workspaceRelative("C:/work/demo2/a.rs", ".", "C:/work/demo")).toBeNull();
    expect(workspaceRelative("../../etc/passwd", ".", "C:/work/demo")).toBeNull();
    expect(workspaceRelative("../shared/a.ts", "web", "C:/work/demo")).toBe("shared/a.ts");
  });

  it("builds editor locations", () => {
    const r = result({ cwd: "web" });
    const ws = run("r", "2026-10-05T10:00:00Z", [r]);
    expect(locationOf({ file: "src/a.ts", line: 12, column: 4 }, r, ws)).toEqual({ path: "web/src/a.ts", line: 12, column: 4 });
    expect(locationOf({ file: "src/a.ts", line: null }, r, ws)).toEqual({ path: "web/src/a.ts", line: 1 });
    expect(locationOf({ file: null, line: 3 }, r, ws)).toBeNull();
  });

  it("extracts run ids from build ids and events", () => {
    expect(runIdOfBuildId("0190-abc:cargo-test")).toBe("0190-abc");
    expect(runIdOfBuildId("nocolon")).toBeNull();
    const ev = {
      seq: 1,
      id: "e",
      ts: "2026-10-05T10:00:00Z",
      kind: "TestRunFinished",
      payload: { runId: "run-1:npm-test", passed: 1, failed: 0, skipped: 0, durationMs: 5, summary: "" },
    } as unknown as Event;
    expect(runIdOfEvent(ev)).toBe("run-1");
    expect(runIdOfEvent({ ...ev, kind: "TaskCreated" } as unknown as Event)).toBeNull();
  });

  it("explains skipped / blocked results", () => {
    expect(resultReason(result({ status: "Skipped", summary: "skipped: the build failed" }))).toBe("buildFailed");
    expect(resultReason(result({ status: "Skipped", summary: "time budget of the run exhausted" }))).toBe("budget");
    expect(resultReason(result({ status: "Blocked", approvalId: "a1", summary: "awaiting approval: x" }))).toBe("approval");
    expect(resultReason(result({ status: "Blocked", summary: "blocked by the security policy: rm" }))).toBe("denied");
    expect(resultReason(result({ status: "Passed" }))).toBeNull();
  });

  it("aggregates totals and the latest status per kind", () => {
    const failingBuild = result({ checkId: "b", kind: "Build", status: "Failed", passed: 0, diagnostics: [{ severity: "Error", message: "x", file: "a.rs", line: 1, column: 1, code: null }] });
    const older = run("old", "2026-10-05T09:00:00Z", [result()]);
    const newer = run("new", "2026-10-05T10:00:00Z", [failingBuild]);
    const totals = runTotals(newer.results);
    expect(totals.errors).toBe(1);
    expect(totals.checksPassed).toBe(0);
    const runs = mergeRun(mergeRun([], older), newer);
    expect(runs.map((r) => r.id)).toEqual(["new", "old"]);
    expect(latestStatus(runs, new Set(["Build"]))?.status).toBe("Failed");
    expect(latestStatus(runs, TEST_KINDS)).toMatchObject({ status: "Passed", run: { id: "old" } });
    expect(latestStatus(runs, new Set(["Lint"]))).toBeNull();
  });

  it("validates profile drafts like the core", () => {
    const base: CheckDef[] = [];
    const a = { ...blankCheck(base), command: "npm test" };
    const b = { ...blankCheck([a]), command: "" };
    expect(b.id).not.toBe(a.id);
    expect(validateProfile([a])).toEqual([]);
    expect(validateProfile([a, b]).map((p) => p.key)).toEqual(["commandRequired"]);
    expect(validateProfile([{ ...a, cwd: "../x" }]).map((p) => p.key)).toEqual(["cwdOutside"]);
    expect(validateProfile([{ ...a, cwd: "C:/x" }]).map((p) => p.key)).toEqual(["cwdOutside"]);
    expect(validateProfile([{ ...a, timeoutSecs: 0 }]).map((p) => p.key)).toEqual(["timeout"]);
    expect(validateProfile([a, { ...a }]).map((p) => p.key)).toEqual(["duplicateId"]);
  });
});

describe("quality UI", () => {
  it("registers commands and the settings section", () => {
    const runTests = commands.find((c) => c.id === "quality.runTests");
    expect(runTests?.shortcut).toBe("Ctrl+Shift+T");
    expect(commands.some((c) => c.id === "quality.runBuild")).toBe(true);
    expect(settingsSections.some((s) => s.id === "quality")).toBe(true);
  });

  it("renders results, expands failures and jumps to the file", () => {
    const failing = result({
      status: "Failed",
      exitCode: 101,
      passed: 2,
      failed: 1,
      failures: [{ name: "tests::adds", message: "assertion failed: left == right", file: "src/lib.rs", line: 42 }],
      rawTail: "test result: FAILED. 2 passed; 1 failed",
    });
    const build = result({ checkId: "build", kind: "Build", status: "Passed", passed: 0, label: "cargo build", command: "cargo build" });
    render(<RunResults run={run("r1", "2026-10-05T10:00:00Z", [build, failing], { status: "Failed" })} />);
    expect(screen.getByText("Passed: 2 · Failed: 1 · Skipped: 0")).toBeTruthy();
    // The first failing check is expanded.
    expect(screen.getByText("tests::adds")).toBeTruthy();
    fireEvent.click(screen.getByTitle("Open src/lib.rs at line 42"));
    const editor = useEditorStore.getState();
    expect(editor.activePath).toBe("src/lib.rs");
    expect(editor.reveal?.line).toBe(42);
    fireEvent.click(screen.getByLabelText("Hide details"));
    expect(screen.queryByText("tests::adds")).toBeNull();
  });

  it("renders in Russian", async () => {
    await act(async () => {
      await i18n.changeLanguage("ru");
    });
    const skipped = result({ status: "Skipped", passed: 0, summary: "skipped: the build failed", durationMs: 0 });
    render(<RunResults run={run("r2", "2026-10-05T10:00:00Z", [skipped], { status: "Failed" })} />);
    expect(screen.getByText("Пропущена: сборка не прошла")).toBeTruthy();
    expect(screen.getByText("Тесты")).toBeTruthy();
  });
});
