import type { RepoStatus, RollbackPlan, StatusEntry, WorkspaceSnapshot } from "@orkestr/protocol";
import { describe, expect, it } from "vitest";

import { commands } from "../../features/commands/registry";
import { getPanel } from "../registry";
import {
  branchNameError,
  buildTree,
  cssColorToHex,
  entryStatus,
  formatBytes,
  groupStatus,
  languageForPath,
  leaves,
  reasonLabel,
  relativeTime,
  statusBadge,
  statusSummary,
  summarizePlan,
} from "./model";

function entry(path: string, index: StatusEntry["index"], worktree: StatusEntry["worktree"], conflict: StatusEntry["conflict"] = null): StatusEntry {
  return { path, origPath: null, index, worktree, conflict, submodule: false };
}

describe("groupStatus", () => {
  it("splits entries into conflicts / staged / unstaged / untracked", () => {
    const g = groupStatus([
      entry("b.ts", "Modified", "Modified"),
      entry("a.ts", "Added", "Unmodified"),
      entry("c.ts", "Unmodified", "Deleted"),
      entry("new file ü.txt", "Untracked", "Untracked"),
      entry("x.log", "Ignored", "Ignored"),
      entry("conf.txt", "Unmerged", "Unmerged", "BothModified"),
    ]);
    expect(g.staged.map((e) => e.path)).toEqual(["a.ts", "b.ts"]);
    expect(g.unstaged.map((e) => e.path)).toEqual(["b.ts", "c.ts"]);
    expect(g.untracked.map((e) => e.path)).toEqual(["new file ü.txt"]);
    expect(g.conflicts.map((e) => e.path)).toEqual(["conf.txt"]);
  });

  it("summarizes status", () => {
    const st = { entries: [entry("a", "Added", "Unmodified"), entry("b", "Untracked", "Untracked")] } as RepoStatus;
    expect(statusSummary(st)).toBe("1 staged · 1 untracked");
    expect(statusSummary({ entries: [] as StatusEntry[] } as RepoStatus)).toBe("Working tree clean");
  });

  it("picks the status of the shown side", () => {
    const e = entry("a", "Added", "Modified");
    expect(entryStatus(e, "staged")).toBe("Added");
    expect(entryStatus(e, "unstaged")).toBe("Modified");
    expect(statusBadge("Deleted")).toMatchObject({ letter: "D", tone: "danger" });
    expect(statusBadge("Untracked").letter).toBe("U");
  });
});

describe("buildTree", () => {
  it("builds a compacted, folder-first tree", () => {
    const items = ["src/app/main.ts", "src/app/util.ts", "README.md", "docs/guide/intro.md", "src/index.ts"];
    const tree = buildTree(items, (p) => p);
    expect(tree.map((n) => n.name)).toEqual(["docs/guide", "src", "README.md"]);
    const src = tree[1];
    expect(src?.children.map((n) => n.name)).toEqual(["app", "index.ts"]);
    expect(src ? leaves(src).sort() : []).toEqual(["src/app/main.ts", "src/app/util.ts", "src/index.ts"]);
    expect(tree[0]?.children[0]?.path).toBe("docs/guide/intro.md");
  });

  it("keeps a file and a folder with the same name apart", () => {
    const tree = buildTree(["a", "a/b"], (p) => p);
    expect(tree).toHaveLength(2);
  });
});

describe("helpers", () => {
  it("maps languages", () => {
    expect(languageForPath("src/a.tsx")).toBe("typescript");
    expect(languageForPath("Cargo.toml")).toBe("ini");
    expect(languageForPath("Dockerfile")).toBe("dockerfile");
    expect(languageForPath("LICENSE")).toBe("plaintext");
  });

  it("formats bytes and times", () => {
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(1536)).toBe("1.5 KB");
    expect(formatBytes(50 * 1024 * 1024)).toBe("50 MB");
    const now = Date.parse("2026-10-02T12:00:00Z");
    expect(relativeTime("2026-10-02T11:59:50Z", now)).toBe("just now");
    expect(relativeTime("2026-10-02T11:30:00Z", now)).toBe("30 min ago");
    expect(relativeTime("2026-10-02T09:00:00Z", now)).toBe("3 h ago");
    expect(relativeTime("2026-09-30T12:00:00Z", now)).toBe("2 d ago");
  });

  it("validates branch names like git", () => {
    expect(branchNameError("feature/login")).toBeNull();
    expect(branchNameError("")).not.toBeNull();
    expect(branchNameError("-x")).not.toBeNull();
    expect(branchNameError("a..b")).not.toBeNull();
    expect(branchNameError("has space")).not.toBeNull();
    expect(branchNameError("x.lock")).not.toBeNull();
    expect(branchNameError("a:b")).not.toBeNull();
  });

  it("converts CSS colors for Monaco", () => {
    expect(cssColorToHex("#ABCDEF", "#000000")).toBe("#abcdef");
    expect(cssColorToHex("#abc", "#000000")).toBe("#aabbcc");
    expect(cssColorToHex("rgb(255, 0, 16)", "#000000")).toBe("#ff0010");
    expect(cssColorToHex("rgba(52, 211, 153, 0.14)", "#000000")).toBe("#34d39924");
    expect(cssColorToHex("rgba(1, 2, 3, 1)", "#000000")).toBe("#010203");
    expect(cssColorToHex("linear-gradient(red, blue)", "#123456")).toBe("#123456");
  });

  it("summarizes rollback plans and labels reasons", () => {
    const plan = {
      items: [
        { action: "Restore" },
        { action: "Restore" },
        { action: "Recreate" },
        { action: "Keep" },
      ],
    } as RollbackPlan;
    expect(summarizePlan(plan)).toEqual({ restore: 2, recreate: 1, delete: 0, keep: 1 });
    expect(reasonLabel({ reason: "TaskCheckpoint", taskPhase: "After" } as WorkspaceSnapshot)).toBe("Task · after");
    expect(reasonLabel({ reason: "PreRollback" } as WorkspaceSnapshot)).toBe("Pre-rollback");
  });
});

describe("wiring", () => {
  it("git panel is available and commands are registered", () => {
    expect(getPanel("git")?.phase).toBe(0);
    const ids = commands.map((c) => c.id);
    expect(ids).toEqual(expect.arrayContaining(["snapshots.create", "snapshots.rollbackWorkspace", "vcs.commit"]));
    const create = commands.find((c) => c.id === "snapshots.create");
    expect(create?.enabled?.({ projectId: null })).toBe(false);
  });
});
