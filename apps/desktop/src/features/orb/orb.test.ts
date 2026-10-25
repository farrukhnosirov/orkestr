/** The orb's pure model: mood, badge, motion policy, hub items, grouping, positions, migration. */
import type { AgentInstance, ClaudeSessionInfo } from "@orkestr/protocol";
import { describe, expect, it } from "vitest";

import { PAUSED_ACTIVITY } from "../../lib/coreText";
import { IDLE_MAIN_STATE, isStateMessage, voicePopupWanted } from "./channel";
import {
  agentItem,
  agentStatus,
  claudeItem,
  groupByProject,
  hubCounts,
  managerRelayText,
  moveSelection,
  RECENT_MS,
  visibleItems,
} from "./hubModel";
import {
  IDLE_SIGNALS,
  type MotionInput,
  badgeText,
  clampOrbSize,
  orbBadge,
  orbBoxSize,
  orbFps,
  orbHitShape,
  orbMood,
  orbMotion,
  orbPrefsOf,
} from "./model";
import { migratedEnabled } from "./OrbHost";
import { parsePositions, rememberPosition } from "./shell";

function agent(over: Partial<AgentInstance>): AgentInstance {
  return {
    id: "a1",
    templateId: "qa",
    projectId: "p1",
    taskId: "t1",
    teamId: null,
    parentAgentId: null,
    name: "QA Agent #1",
    role: "Qa",
    state: "Coding",
    model: null,
    worktree: null,
    usage: {} as never,
    turnsUsed: 5,
    lastError: null,
    createdAt: "2026-10-23T10:00:00Z",
    startedAt: "2026-10-23T10:00:00Z",
    stoppedAt: null,
    lastActivityAt: "2026-10-23T10:05:00Z",
    templateVersion: null,
    worktreePolicy: "Auto" as never,
    maxTurns: 20,
    budget: null,
    executionId: null,
    executionStyle: null,
    cliSession: null,
    activity: "Running cargo test",
    ...over,
  } as AgentInstance;
}

function session(over: Partial<ClaudeSessionInfo>): ClaudeSessionInfo {
  return {
    sessionId: "s1",
    projectId: "p2",
    cwd: "C:/p2",
    claudeSessionId: "c1",
    model: null,
    permissionMode: "default",
    bypassAllowed: false,
    state: "running",
    title: "Fix the login bug",
    pid: 1,
    startedAt: "2026-10-23T10:00:00Z",
    error: null,
    ...over,
  } as ClaudeSessionInfo;
}

describe("orb mood and badge", () => {
  it("picks the most urgent mood", () => {
    expect(orbMood(IDLE_SIGNALS)).toBe("idle");
    expect(orbMood({ ...IDLE_SIGNALS, working: 2 })).toBe("working");
    expect(orbMood({ ...IDLE_SIGNALS, working: 2, errors: 1 })).toBe("error");
    expect(orbMood({ ...IDLE_SIGNALS, working: 2, errors: 1, approvals: 1 })).toBe("approval");
    expect(orbMood({ ...IDLE_SIGNALS, approvals: 1, allStopped: true })).toBe("stopped");
    expect(orbMood({ ...IDLE_SIGNALS, approvals: 1, speaking: true })).toBe("speaking");
    expect(orbMood({ ...IDLE_SIGNALS, speaking: true, voicePhase: "listening" })).toBe("listening");
    expect(orbMood({ ...IDLE_SIGNALS, voicePhase: "transcribing" })).toBe("listening");
    expect(orbMood({ ...IDLE_SIGNALS, voicePhase: "thinking", working: 1 })).toBe("working");
    expect(orbMood({ ...IDLE_SIGNALS, connected: false, approvals: 3 })).toBe("offline");
  });

  it("badges approvals, then errors, then working agents", () => {
    expect(orbBadge({ ...IDLE_SIGNALS, working: 3, approvals: 2 }, true)).toEqual({ count: 2, tone: "warning" });
    expect(orbBadge({ ...IDLE_SIGNALS, working: 3, errors: 1 }, true)).toEqual({ count: 1, tone: "danger" });
    expect(orbBadge({ ...IDLE_SIGNALS, working: 3 }, true)).toEqual({ count: 3, tone: "accent" });
    expect(orbBadge(IDLE_SIGNALS, true)).toBeNull();
    expect(orbBadge({ ...IDLE_SIGNALS, working: 3 }, false)).toBeNull();
    expect(badgeText(7)).toBe("7");
    expect(badgeText(140)).toBe("99+");
  });
});

describe("orb motion follows Performance Mode and focus", () => {
  const base: MotionInput = { pref: "Full", mode: "Balanced", reducedMotion: false, hidden: false, appActive: true, hovered: false, hubOpen: false, voice: false };
  it("is full in the visual modes while Orkestr is in use", () => {
    expect(orbMotion(base)).toBe("full");
    expect(orbMotion({ ...base, mode: "UltraVisual" })).toBe("full");
    expect(orbFps("full", "Balanced")).toBe(30);
  });
  it("is reduced in Performance mode, with reduced motion or by choice", () => {
    expect(orbMotion({ ...base, mode: "Performance" })).toBe("reduced");
    expect(orbMotion({ ...base, reducedMotion: true })).toBe("reduced");
    expect(orbMotion({ ...base, pref: "Reduced" })).toBe("reduced");
    expect(orbFps("reduced", "UltraVisual")).toBe(15);
  });
  it("is still in Battery Saver, when hidden, by choice, and while another app is focused", () => {
    expect(orbMotion({ ...base, mode: "BatterySaver" })).toBe("static");
    expect(orbMotion({ ...base, hidden: true })).toBe("static");
    expect(orbMotion({ ...base, pref: "Static" })).toBe("static");
    expect(orbMotion({ ...base, appActive: false })).toBe("static");
    expect(orbFps("static", "UltraVisual")).toBe(0);
  });
  it("wakes up on hover, with the hub open, and for live voice", () => {
    expect(orbMotion({ ...base, appActive: false, hovered: true })).toBe("full");
    expect(orbMotion({ ...base, appActive: false, hubOpen: true })).toBe("full");
    expect(orbMotion({ ...base, appActive: false, voice: true })).toBe("full");
    expect(orbMotion({ ...base, appActive: false, voice: true, mode: "BatterySaver" })).toBe("static");
  });
});

describe("orb geometry and prefs", () => {
  it("clamps the size and computes the box", () => {
    expect(clampOrbSize(10)).toBe(48);
    expect(clampOrbSize(200)).toBe(96);
    expect(clampOrbSize(Number.NaN)).toBe(64);
    expect(orbBoxSize(64)).toBe(88);
    expect(orbPrefsOf(null)).toEqual({ enabled: true, size: 64, showBadge: true, animation: "Full", voiceSize: 150 });
    expect(orbPrefsOf({ voiceSize: 999 }).voiceSize).toBe(180);
    expect(orbPrefsOf({ size: 500, enabled: false }).size).toBe(96);
  });
  it("hit shape is the sphere circle plus open panels", () => {
    const shape = orbHitShape({ x: 0, y: 0 }, 64, [null, { x: 96, y: 0, w: 380, h: 540 }, { x: 0, y: 0, w: 0, h: 10 }]);
    expect(shape.circles).toEqual([{ cx: 44, cy: 44, r: 35 }]);
    expect(shape.rects).toEqual([{ x: 96, y: 0, w: 380, h: 540 }]);
  });
  it("remembers one position per monitor, most recent first", () => {
    let list = rememberPosition([], { monitor: "A", x: 1, y: 2 });
    list = rememberPosition(list, { monitor: "B", x: 3, y: 4 });
    list = rememberPosition(list, { monitor: "A", x: 5, y: 6 });
    expect(list).toEqual([
      { monitor: "A", x: 5, y: 6 },
      { monitor: "B", x: 3, y: 4 },
    ]);
    expect(parsePositions(JSON.stringify(list))).toEqual(list);
    expect(parsePositions("garbage")).toEqual([]);
    expect(parsePositions(JSON.stringify([{ monitor: 1, x: 1, y: 1 }, { monitor: null, x: 2, y: "no" }]))).toEqual([]);
  });
  it("migrates robots switched off to the orb switched off", () => {
    expect(migratedEnabled({ enabled: false })).toBe(false);
    expect(migratedEnabled({ enabled: true })).toBeNull();
    expect(migratedEnabled(null)).toBeNull();
  });
  it("accepts only well-formed channel state", () => {
    expect(isStateMessage({ type: "state", state: { appActive: true, voice: { phase: "idle" } } })).toBe(true);
    expect(isStateMessage({ type: "state", state: { appActive: "yes", voice: {} } })).toBe(false);
    expect(isStateMessage({ type: "ptt", down: true })).toBe(false);
    expect(isStateMessage(null)).toBe(false);
  });
});

describe("hub items", () => {
  it("normalizes agent states and offers the right actions", () => {
    expect(agentStatus(agent({ state: "Coding" }))).toBe("working");
    expect(agentStatus(agent({ state: "Waiting" }))).toBe("waiting");
    expect(agentStatus(agent({ state: "Idle", activity: PAUSED_ACTIVITY }))).toBe("paused");
    expect(agentStatus(agent({ state: "Idle", stoppedAt: "x" }))).toBe("stopped");
    expect(agentStatus(agent({ state: "Error" }))).toBe("error");
    expect(agentStatus(agent({ state: "Completed" }))).toBe("done");

    const running = agentItem(agent({}));
    expect(running.actions).toEqual(["message", "pause", "stop", "transcript"]);
    expect(running.progress).toBe(0.25);
    expect(agentItem(agent({ state: "Idle", activity: PAUSED_ACTIVITY })).actions).toEqual(["message", "resume", "stop", "transcript"]);
    expect(agentItem(agent({ state: "Error" })).actions).toEqual(["message", "restart", "transcript"]);
    expect(agentItem(agent({ state: "Completed", templateId: null })).actions).toEqual(["message", "transcript"]);
    expect(agentItem(agent({ maxTurns: 0 })).progress).toBeNull();
  });

  it("maps Claude Code sessions", () => {
    expect(claudeItem(session({})).status).toBe("working");
    expect(claudeItem(session({})).actions).toEqual(["message", "stop", "transcript"]);
    expect(claudeItem(session({ state: "waitingPermission" })).status).toBe("waiting");
    expect(claudeItem(session({ state: "exited" })).actions).toEqual(["transcript"]);
    expect(claudeItem(session({ title: "  " })).name).toBe("Claude Code");
  });

  it("keeps live items and recently finished ones", () => {
    const now = Date.parse("2026-10-23T12:00:00Z");
    const old = agentItem(agent({ id: "old", state: "Completed", lastActivityAt: new Date(now - RECENT_MS - 1000).toISOString() }));
    const recent = agentItem(agent({ id: "recent", state: "Completed", lastActivityAt: new Date(now - 60_000).toISOString() }));
    const live = agentItem(agent({ id: "live", lastActivityAt: "2020-01-01T00:00:00Z" }));
    expect(visibleItems([old, recent, live], now).map((i) => i.id)).toEqual(["recent", "live"]);
  });

  it("groups by project, attention first, and counts", () => {
    const items = [
      agentItem(agent({ id: "a", projectId: "p1", state: "Coding" })),
      agentItem(agent({ id: "b", projectId: "p1", state: "Completed" })),
      claudeItem(session({ sessionId: "s", projectId: "p2", state: "waitingPermission" })),
      agentItem(agent({ id: "e", projectId: "p3", state: "Error" })),
    ];
    const groups = groupByProject(items, [
      { id: "p1", name: "Alpha", color: null },
      { id: "p2", name: "Beta", color: "#f00" },
    ]);
    expect(groups.map((g) => g.name)).toEqual(["Beta", "p3", "Alpha"]);
    expect(groups[2]?.items.map((i) => i.id)).toEqual(["a", "b"]);
    expect(hubCounts(items)).toEqual({ working: 1, waiting: 1, errors: 1 });
    expect(hubCounts(items, new Set(["e"])).errors).toBe(0);
  });

  it("relays agent messages through the Manager and moves the selection", () => {
    expect(managerRelayText("QA", "a1", "  run the tests again ")).toBe('[Message for agent "QA" (a1)] run the tests again');
    expect(moveSelection(["a", "b", "c"], null, 1)).toBe("a");
    expect(moveSelection(["a", "b", "c"], null, -1)).toBe("c");
    expect(moveSelection(["a", "b", "c"], "b", 1)).toBe("c");
    expect(moveSelection(["a", "b", "c"], "c", 1)).toBe("c");
    expect(moveSelection([], "a", 1)).toBeNull();
  });
});

describe("voice popup next to the orb", () => {
  const v = (phase: string) => ({ ...IDLE_MAIN_STATE.voice, enabled: true, phase });
  it("opens for a command while Orkestr is in the background, stays until it is done", () => {
    expect(voicePopupWanted(v("listening"), false, false)).toBe(true);
    expect(voicePopupWanted(v("listening"), true, false)).toBe(false);
    expect(voicePopupWanted(v("result"), true, true)).toBe(true);
    expect(voicePopupWanted(v("idle"), false, true)).toBe(false);
  });
});
