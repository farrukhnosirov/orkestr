import type { AgentInstance, Approval, ManagerRun } from "@orkestr/protocol";
import { describe, expect, it } from "vitest";

import { agentStrip, isQuickApprovable, nextApproval, parsePosition, runLine, serializePosition } from "./hudModel";

const agent = (id: string, state: AgentInstance["state"], name = id): AgentInstance => ({ id, name, state }) as AgentInstance;
const run = (id: string, status: ManagerRun["status"], updatedAt: string, done = 0, total = 3): ManagerRun =>
  ({
    id,
    status,
    updatedAt,
    goal: `goal ${id}`,
    tasks: Array.from({ length: total }, (_, i) => ({ status: i < done ? "Completed" : "Running" })),
  }) as ManagerRun;
const approval = (id: string, risk: Approval["risk"], requestedAt: string, danger: Approval["danger"] = null): Approval =>
  ({ id, risk, requestedAt, danger, status: "Pending", actionSummary: id }) as Approval;

describe("HUD model", () => {
  it("strip: working agents and errors, busiest first, overflow counted", () => {
    const list = [agent("a", "Idle"), agent("b", "Thinking"), agent("c", "Coding"), agent("d", "Error"), agent("e", "Completed"), agent("f", "Testing")];
    const s = agentStrip(list, 2);
    expect(s.shown.map((a) => a.id)).toEqual(["c", "f"]);
    expect(s.more).toBe(2); // b + d
    expect(s.working).toBe(3);
    expect(agentStrip([]).shown).toEqual([]);
  });

  it("run line: live run wins; old finished runs fall back to idle", () => {
    const now = Date.parse("2026-10-05T12:00:00Z");
    expect(runLine([], now)).toEqual({ kind: "idle" });
    const live = run("live", "Running", "2026-10-05T11:00:00Z", 1, 4);
    const done = run("done", "Completed", "2026-10-05T11:59:00Z", 3, 3);
    expect(runLine([done, live], now)).toEqual({ kind: "run", status: "Running", done: 1, total: 4, goal: "goal live" });
    expect(runLine([done], now)).toMatchObject({ kind: "run", status: "Completed", done: 3 });
    expect(runLine([run("old", "Failed", "2026-10-05T10:00:00Z")], now)).toEqual({ kind: "idle" });
  });

  it("quick approve only for low/medium risk without a danger category", () => {
    expect(isQuickApprovable(approval("a", "Low", ""))).toBe(true);
    expect(isQuickApprovable(approval("a", "Medium", ""))).toBe(true);
    expect(isQuickApprovable(approval("a", "High", ""))).toBe(false);
    expect(isQuickApprovable(approval("a", "Critical", ""))).toBe(false);
    expect(isQuickApprovable(approval("a", "Low", "", "DestructiveFs" as Approval["danger"]))).toBe(false);
    const list = [approval("new", "Low", "2026-10-05T11:00:00Z"), approval("old", "High", "2026-10-05T10:00:00Z")];
    expect(nextApproval(list)?.id).toBe("old");
    expect(nextApproval([])).toBeNull();
  });

  it("position memory round-trips and rejects garbage", () => {
    expect(parsePosition(serializePosition({ x: 2400.4, y: -10.6 }))).toEqual({ x: 2400, y: -11 });
    for (const bad of [null, "", "nope", "[]", '{"x":"1","y":2}', '{"x":1}', '{"x":1e9,"y":0}', "null"]) {
      expect(parsePosition(bad)).toBeNull();
    }
  });
});
