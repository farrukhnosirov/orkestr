import type { Event, EventKind, VoiceNotificationConfig } from "@orkestr/protocol";
import { describe, expect, it } from "vitest";

import { inQuietHours, noticeFromEvent, phraseId, PhraseLimiter, speakDecision, type VoiceNotice } from "./policy";

let seq = 0;
function ev(kind: EventKind): Event {
  seq += 1;
  return {
    seq,
    id: `e${seq}`,
    ts: "2026-10-05T10:00:00Z",
    projectId: "p1",
    taskId: null,
    agentId: null,
    nodeId: null,
    correlationId: null,
    ...kind,
  } as Event;
}

const CONFIG: VoiceNotificationConfig = {
  enabled: true,
  quietHours: { enabled: false, start: "22:00", end: "08:00" },
  taskCompleted: true,
  taskFailed: true,
  approvalNeeded: true,
  buildFailed: true,
  budgetThreshold: true,
  deployFinished: true,
};

const NOON = new Date(2026, 9, 5, 12, 0);

describe("noticeFromEvent", () => {
  it("maps key events to notices with cues", () => {
    const done = noticeFromEvent(ev({ kind: "TaskStatusChanged", payload: { taskId: "t", from: "Running", to: "Completed", reason: null } }));
    expect(done).toMatchObject({ key: "taskCompleted", cue: "completion", projectId: "p1" });
    const failed = noticeFromEvent(ev({ kind: "TaskStatusChanged", payload: { taskId: "t", from: "Running", to: "Failed", reason: "x" } }));
    expect(failed).toMatchObject({ key: "taskFailed", cue: "error" });
    expect(noticeFromEvent(ev({ kind: "TaskStatusChanged", payload: { taskId: "t", from: "Ready", to: "Running", reason: null } }))).toBeNull();
    const build = noticeFromEvent(ev({ kind: "BuildFailed", payload: { buildId: "b", exitCode: 1, durationMs: 1, summary: "" } }));
    expect(build).toMatchObject({ key: "buildFailed", cue: "error" });
  });

  it("budget: warning vs. over the limit (critical)", () => {
    const warn = noticeFromEvent(ev({ kind: "BudgetThreshold", payload: { scope: "project", spentUsd: 8, limitUsd: 10, fraction: 0.8 } }));
    expect(warn).toMatchObject({ key: "budgetThreshold", variant: "warn", critical: false, vars: { percent: 80 } });
    const over = noticeFromEvent(ev({ kind: "BudgetThreshold", payload: { scope: "project", spentUsd: 11, limitUsd: 10, fraction: 1.1 } }));
    expect(over).toMatchObject({ variant: "over", critical: true });
    expect(over && phraseId(over)).toBe("budgetOver");
    expect(warn && phraseId(warn)).toBe("budgetWarn");
  });

  it("ignores unrelated events", () => {
    expect(noticeFromEvent(ev({ kind: "Log", payload: { level: "Info", target: "x", message: "m" } } as unknown as EventKind))).toBeNull();
  });
});

describe("inQuietHours", () => {
  const q = (start: string, end: string) => ({ enabled: true, start, end });
  it("handles ranges that wrap past midnight", () => {
    expect(inQuietHours(q("22:00", "08:00"), new Date(2026, 0, 1, 23, 30))).toBe(true);
    expect(inQuietHours(q("22:00", "08:00"), new Date(2026, 0, 1, 7, 59))).toBe(true);
    expect(inQuietHours(q("22:00", "08:00"), new Date(2026, 0, 1, 8, 0))).toBe(false);
    expect(inQuietHours(q("22:00", "08:00"), NOON)).toBe(false);
  });
  it("handles same-day ranges, disabled and degenerate ranges", () => {
    expect(inQuietHours(q("12:00", "13:00"), NOON)).toBe(true);
    expect(inQuietHours(q("13:00", "14:00"), NOON)).toBe(false);
    expect(inQuietHours({ enabled: false, start: "00:00", end: "23:59" }, NOON)).toBe(false);
    expect(inQuietHours(q("10:00", "10:00"), NOON)).toBe(false);
    expect(inQuietHours(q("bad", "10:00"), NOON)).toBe(false);
  });
});

describe("speakDecision", () => {
  const notice = (critical = false): VoiceNotice => ({
    id: "x",
    key: "taskCompleted",
    variant: "default",
    cue: "completion",
    critical,
    projectId: null,
    agentId: null,
    vars: {},
  });
  const ctx = { config: CONFIG, voiceEnabled: true, doNotDisturb: false, now: NOON };

  it("speaks by default", () => expect(speakDecision(notice(), ctx)).toBe("speak"));
  it("respects master switches and per-event toggles", () => {
    expect(speakDecision(notice(), { ...ctx, voiceEnabled: false })).toBe("disabled");
    expect(speakDecision(notice(), { ...ctx, config: { ...CONFIG, enabled: false } })).toBe("disabled");
    expect(speakDecision(notice(), { ...ctx, config: { ...CONFIG, taskCompleted: false } })).toBe("eventOff");
    expect(speakDecision(notice(true), { ...ctx, config: { ...CONFIG, taskCompleted: false } })).toBe("eventOff");
  });
  it("is silent in DND and quiet hours, except critical notices", () => {
    expect(speakDecision(notice(), { ...ctx, doNotDisturb: true })).toBe("dnd");
    const quiet = { ...CONFIG, quietHours: { enabled: true, start: "11:00", end: "13:00" } };
    expect(speakDecision(notice(), { ...ctx, config: quiet })).toBe("quietHours");
    expect(speakDecision(notice(true), { ...ctx, doNotDisturb: true })).toBe("speak");
    expect(speakDecision(notice(true), { ...ctx, config: quiet })).toBe("speak");
  });
});

describe("PhraseLimiter", () => {
  it("limits bursts and refuses duplicates", () => {
    const l = new PhraseLimiter(2, 10_000);
    expect(l.admit("a", 0)).toBe(true);
    expect(l.admit("a", 1)).toBe(false);
    expect(l.admit("b", 2)).toBe(true);
    expect(l.admit("c", 3)).toBe(false);
    expect(l.admit("d", 4)).toBe(false);
    expect(l.takeDropped()).toBe(2);
    expect(l.takeDropped()).toBe(0);
    expect(l.admit("e", 10_001)).toBe(true);
  });
});
