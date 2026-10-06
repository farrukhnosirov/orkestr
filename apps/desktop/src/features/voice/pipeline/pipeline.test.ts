import type { AgentInstance, Approval, Task } from "@orkestr/protocol";
import { beforeAll, describe, expect, it, vi } from "vitest";

import { i18n } from "../../../i18n";
import { type IntentContext, needsConfirmation, parseIntent } from "../intent/parser";
import { bindApproval, confirmationPrompt, executeIntent, precheck, targetApproval, type VoiceDeps } from "./executor";
import { isBusy, nextPhase } from "./machine";
import { projectFacts, speakable, summaryParts } from "./summary";

beforeAll(async () => {
  await i18n.changeLanguage("en");
});

const NOW = new Date(2026, 9, 5, 12, 0);

function task(status: Task["status"], finishedAt: string | null = null): Task {
  return { id: `t-${Math.random()}`, status, finishedAt } as unknown as Task;
}
function agent(id: string, state: AgentInstance["state"], activity: string | null = null, projectId = "p1"): AgentInstance {
  return { id, name: `Agent ${id}`, state, activity, projectId, lastActivityAt: "2026-10-05T11:00:00Z" } as unknown as AgentInstance;
}
function approval(id: string, projectId: string | null, requestedAt: string, status: Approval["status"] = "Pending"): Approval {
  return { id, projectId, requestedAt, status, actionSummary: `action ${id}` } as unknown as Approval;
}

const CTX: IntentContext = {
  projects: [
    { id: "p1", name: "Schoolify" },
    { id: "p2", name: "Thompson Finance" },
  ],
  agents: [{ id: "a1", name: "Backend Agent #1", projectId: "p1" }],
  activeProjectId: "p2",
  wakePhrases: ["Jarvis"],
};

function fakeDeps(over: Partial<VoiceDeps> = {}): VoiceDeps & { calls: string[] } {
  const calls: string[] = [];
  const d: VoiceDeps = {
    activeProjectId: () => "p2",
    projectName: (id) => CTX.projects.find((p) => p.id === id)?.name ?? null,
    listTasks: async () => [task("Running"), task("Failed"), task("Completed", NOW.toISOString())],
    listAgents: async () => [agent("a1", "Coding", "writing the API")],
    pendingApprovals: () => [],
    resolveApproval: async (id, d2) => void calls.push(`resolve:${id}:${d2}`),
    stopProject: async (id) => {
      calls.push(`stopProject:${id}`);
      return 2;
    },
    stopAll: async () => {
      calls.push("stopAll");
      return true;
    },
    openPanel: (p) => void calls.push(`open:${p}`),
    switchProject: async (id) => void calls.push(`switch:${id}`),
    setDnd: async (on) => {
      calls.push(`dnd:${on}`);
      return true;
    },
    askManager: async (pid, text) => {
      calls.push(`ask:${pid}:${text}`);
      return { replyMessageId: "m1" };
    },
    buildErrors: async () => [],
    quality: async () => ({ lastBuild: null, lastTests: null, openReviews: null }),
    ...over,
  };
  return Object.assign(d, { calls });
}

describe("state machine", () => {
  it("push-to-talk happy path", () => {
    expect(nextPhase("idle", { type: "listen" })).toBe("listening");
    expect(nextPhase("listening", { type: "speechEnd" })).toBe("transcribing");
    expect(nextPhase("transcribing", { type: "transcript", text: "x" })).toBe("thinking");
    expect(nextPhase("thinking", { type: "done" })).toBe("result");
    expect(nextPhase("result", { type: "dismiss" })).toBe("idle");
  });
  it("confirmation, cancel, failure and invalid events", () => {
    expect(nextPhase("thinking", { type: "needsConfirm" })).toBe("confirm");
    expect(nextPhase("confirm", { type: "confirmed" })).toBe("thinking");
    expect(nextPhase("confirm", { type: "listen" })).toBe("listening");
    expect(nextPhase("listening", { type: "cancel" })).toBe("idle");
    expect(nextPhase("transcribing", { type: "fail" })).toBe("error");
    expect(nextPhase("idle", { type: "fail" })).toBeNull();
    expect(nextPhase("thinking", { type: "listen" })).toBeNull();
    expect(nextPhase("idle", { type: "speechEnd" })).toBeNull();
    expect(nextPhase("idle", { type: "typed", text: "x" })).toBe("thinking");
    expect(isBusy("thinking") && isBusy("transcribing") && !isBusy("idle")).toBe(true);
  });
});

describe("summary", () => {
  it("computes facts and orders urgent items last but keeps them", () => {
    const f = projectFacts(
      [task("Running"), task("Testing"), task("Failed"), task("Blocked"), task("Completed", NOW.toISOString()), task("Completed", "2026-01-01T00:00:00Z")],
      [agent("a1", "Coding", "API"), agent("a2", "Idle")],
      1,
      NOW,
    );
    expect(f).toMatchObject({ running: 2, failed: 1, blocked: 1, completedToday: 1, activeAgents: 1, approvals: 1 });
    const ids = summaryParts(f).map((p) => p.id);
    expect(ids).toHaveLength(4);
    expect(ids).toContain("approvals");
    expect(ids).toContain("failed");
  });
  it("idle and empty projects", () => {
    expect(summaryParts(projectFacts([], [], 0, NOW)).map((p) => p.id)).toEqual(["noTasks"]);
    expect(summaryParts(projectFacts([task("Completed")], [], 0, NOW)).map((p) => p.id)).toEqual(["idle"]);
  });
  it("speakable trims markdown and long text at sentence ends", () => {
    expect(speakable("**Done**. See `x`.")).toBe("Done . See x.");
    const long = "First sentence here. Second sentence is longer and goes on. Third one.";
    expect(speakable(long, 30)).toBe("First sentence here.");
    expect(speakable("a".repeat(50), 20).endsWith("…")).toBe(true);
  });
});

describe("executor", () => {
  it("G: 'Jarvis, что происходит со Schoolify?' → concise status of that project", async () => {
    const intent = parseIntent("Jarvis, что происходит со Schoolify?", CTX);
    expect(intent.kind).toBe("projectStatus");
    const deps = fakeDeps();
    const r = await executeIntent(intent, deps);
    expect(r.outcome).toBe("ok");
    expect(r.projectId).toBe("p1");
    expect(r.reply.startsWith("Schoolify:")).toBe(true);
    expect(r.reply).toContain("1 task in progress");
    expect(r.reply).toContain("1 task failed");
    expect(r.tone).toBe("warn");
  });

  it("stop project needs confirmation, then calls emergency.stopProject", async () => {
    const intent = parseIntent("останови Thompson Finance", CTX);
    expect(needsConfirmation(intent)).toBe(true);
    const deps = fakeDeps();
    expect(confirmationPrompt(intent, deps)).toContain("Thompson Finance");
    const r = await executeIntent(intent, deps);
    expect(deps.calls).toEqual(["stopProject:p2"]);
    expect(r.reply).toContain("2 agents");
  });

  it("unknown project fails before confirmation", () => {
    const intent = parseIntent("stop project Zebra Unicorn", CTX);
    const pre = precheck(intent, fakeDeps());
    expect(pre?.outcome).toBe("failed");
  });

  it("run QA without a quality runner dependency goes to the Manager", async () => {
    const deps = fakeDeps();
    const r = await executeIntent(parseIntent("запусти QA по Schoolify", CTX), deps);
    expect(r.outcome).toBe("sentToManager");
    expect(r.managerReply).toEqual({ projectId: "p1", messageId: "m1" });
    expect(deps.calls[0]?.startsWith("ask:p1:")).toBe(true);
    const runQa = vi.fn(async () => {});
    const r2 = await executeIntent(parseIntent("run qa on schoolify", CTX), fakeDeps({ runQa }));
    expect(runQa).toHaveBeenCalledWith("p1");
    expect(r2.outcome).toBe("ok");
  });

  it("agent status reports state and activity", async () => {
    const r = await executeIntent(parseIntent("что делает Backend Agent", CTX), fakeDeps());
    expect(r.reply).toContain("writing the API");
  });

  it("build errors open the log and summarize the latest failure", async () => {
    const deps = fakeDeps({ buildErrors: async () => [{ at: "2026-10-05T10:00:00Z", summary: "tsc: 3 errors", exitCode: 2 }] });
    const r = await executeIntent(parseIntent("покажи ошибки сборки", CTX), deps);
    expect(deps.calls).toContain("open:logs");
    expect(r.reply).toContain("tsc: 3 errors");
    expect(r.tone).toBe("error");
  });

  it("approvals: picks the newest pending one of the active project", async () => {
    const pending = [approval("x1", "p1", "2026-10-05T10:00:00Z"), approval("x2", "p2", "2026-10-05T09:00:00Z"), approval("x3", "p2", "2026-10-05T11:00:00Z", "Approved")];
    expect(targetApproval(pending, "p2")?.id).toBe("x2");
    expect(targetApproval(pending, null)?.id).toBe("x1");
    const deps = fakeDeps({ pendingApprovals: () => pending });
    await executeIntent(parseIntent("approve", CTX), deps);
    expect(deps.calls).toEqual(["resolve:x2:Approve"]);
    expect(precheck(parseIntent("reject", CTX), fakeDeps())?.outcome).toBe("failed");
  });

  it("approvals: a confirmed 'yes' resolves exactly the approval that was read out", async () => {
    let pending = [approval("x1", "p2", "2026-10-05T10:00:00Z")];
    const deps = fakeDeps({ pendingApprovals: () => pending });
    const bound = bindApproval(parseIntent("approve", CTX), deps);
    expect(bound).toEqual({ kind: "approve", approvalId: "x1" });
    expect(confirmationPrompt(bound, deps)).toContain("action x1");
    // A newer approval (e.g. a force push) arrives while the prompt is spoken.
    pending = [...pending, approval("x9", "p2", "2026-10-05T10:00:05Z")];
    expect(confirmationPrompt(bound, deps)).toContain("action x1");
    const r = await executeIntent(bound, deps);
    expect(deps.calls).toEqual(["resolve:x1:Approve"]);
    expect(r.reply).toContain("action x1");
  });

  it("approvals: a bound approval resolved elsewhere is not replaced by another one", async () => {
    let pending = [approval("x1", "p2", "2026-10-05T10:00:00Z"), approval("x0", "p2", "2026-10-05T09:00:00Z")];
    const deps = fakeDeps({ pendingApprovals: () => pending });
    const bound = bindApproval(parseIntent("reject", CTX), deps);
    pending = [approval("x1", "p2", "2026-10-05T10:00:00Z", "Approved"), approval("x0", "p2", "2026-10-05T09:00:00Z")];
    const r = await executeIntent(bound, deps);
    expect(r.outcome).toBe("failed");
    expect(deps.calls).toEqual([]);
  });

  it("approvals: dangerous actions are never approved by voice (reject still works)", async () => {
    const danger = { ...approval("d1", "p2", "2026-10-05T10:00:00Z"), danger: "GitDestructive" } as unknown as Approval;
    const deps = fakeDeps({ pendingApprovals: () => [danger] });
    const approve = bindApproval(parseIntent("approve", CTX), deps);
    expect(precheck(approve, deps)?.outcome).toBe("failed");
    expect((await executeIntent(approve, deps)).outcome).toBe("failed");
    expect(deps.calls).toEqual([]);
    await executeIntent(bindApproval(parseIntent("reject", CTX), deps), deps);
    expect(deps.calls).toEqual(["resolve:d1:Reject"]);
  });

  it("open panel, switch project, DND, ask manager", async () => {
    const deps = fakeDeps();
    await executeIntent(parseIntent("открой терминал", CTX), deps);
    await executeIntent(parseIntent("переключись на Schoolify", CTX), deps);
    await executeIntent(parseIntent("включи не беспокоить", CTX), deps);
    const r = await executeIntent(parseIntent("напиши план миграции базы", CTX), deps);
    expect(deps.calls.slice(0, 3)).toEqual(["open:terminal", "switch:p1", "dnd:true"]);
    expect(r.outcome).toBe("sentToManager");
    const noProject = await executeIntent({ kind: "askManager", text: "x" }, fakeDeps({ activeProjectId: () => null }));
    expect(noProject.outcome).toBe("notUnderstood");
  });

  it("stop all after confirmation and emergency stop at once", async () => {
    const deps = fakeDeps();
    expect(needsConfirmation(parseIntent("stop all", CTX))).toBe(true);
    expect(needsConfirmation(parseIntent("emergency stop", CTX))).toBe(false);
    await executeIntent(parseIntent("emergency stop", CTX), deps);
    expect(deps.calls).toEqual(["stopAll"]);
  });
});
