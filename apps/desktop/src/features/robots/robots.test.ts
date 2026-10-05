import type { AgentInstance, AgentState, Event, RobotsConfig } from "@orkestr/protocol";
import { describe, expect, it } from "vitest";

import { planAssign } from "./model/assign";
import { bubbleFor, bubbleForEvent, initialBubble, mayReplace, passesVerbosity } from "./model/bubble";
import { accentOf, defaultDesign, defaultRobotsConfig, normalizeDesign, toggleAccessory, validateDesign } from "./model/design";
import { collectDropZones, hitShapeFor, hitTest, isDragGesture, robotWindowLayout } from "./model/layout";
import { isRunning, nextEnabled, robotMenu } from "./model/menu";
import { robotEffects, robotFps, shouldRender } from "./model/perf";
import { blink, poseAt } from "./model/pose";
import { parseRobotRoute } from "./model/route";
import { isBusy, previewStateAt, ROBOT_STATES, statePriority, visualStateFor } from "./model/states";
import {
  applyRobotEvent,
  diffWindows,
  mergeRobotEntries,
  nextExpiry,
  pruneSlots,
  type RobotSlot,
  type RobotSlots,
  selectRobots,
} from "./model/visibility";
import { clusterOpenArgs, robotOpenArgs, staleWindows } from "./model/windows";

const PROJECT = "0190f0b0-0000-7000-8000-ffffffffffff";
const OTHER_PROJECT = "0190f0b0-0000-7000-8000-eeeeeeeeeeee";
const TEMPLATE = "0190f0b0-0000-7000-8000-0000000000aa";

function agent(id: string, patch: Partial<AgentInstance> = {}): AgentInstance {
  return {
    id,
    templateId: TEMPLATE,
    projectId: PROJECT,
    taskId: null,
    teamId: null,
    parentAgentId: null,
    name: `QA ${id.slice(-2)}`,
    role: "Qa",
    state: "Thinking",
    model: null,
    worktree: null,
    usage: { inputTokens: 0, outputTokens: 0, costUsd: 0 },
    turnsUsed: 0,
    lastError: null,
    createdAt: "2026-10-05T10:00:00Z",
    startedAt: "2026-10-05T10:00:00Z",
    stoppedAt: null,
    lastActivityAt: null,
    templateVersion: "1.0.0",
    worktreePolicy: "DedicatedWorktree",
    maxTurns: 40,
    budget: null,
    executionId: null,
    executionStyle: "chat",
    cliSession: null,
    activity: null,
    ...patch,
  } as AgentInstance;
}

function ev(kind: string, payload: unknown, agentId: string | null = null): Event {
  return {
    seq: 1,
    id: "e",
    ts: "2026-10-05T10:00:00Z",
    projectId: PROJECT,
    taskId: null,
    agentId,
    nodeId: null,
    correlationId: null,
    kind,
    payload,
  } as Event;
}

const stateChanged = (agentId: string, from: AgentState, to: AgentState, activity: string | null = null) =>
  ev("AgentStateChanged", { agentId, from, to, activity }, agentId);

describe("robot states (§30)", () => {
  it("maps every agent state to a robot state; Stopped sleeps", () => {
    const agentStates: AgentState[] = ["Idle", "Listening", "Thinking", "Coding", "Testing", "Reviewing", "Waiting", "Error", "Completed", "Stopped"];
    for (const s of agentStates) {
      const v = visualStateFor(s);
      expect(ROBOT_STATES).toContain(v.state);
      expect(v.asleep).toBe(s === "Stopped");
      if (s !== "Stopped") expect(v.state).toBe(s);
    }
    expect(visualStateFor("Stopped")).toEqual({ state: "Idle", asleep: true });
  });

  it("orders problems before results before work before idle", () => {
    expect(statePriority("Error")).toBeLessThan(statePriority("Waiting"));
    expect(statePriority("Waiting")).toBeLessThan(statePriority("Completed"));
    expect(statePriority("Completed")).toBeLessThan(statePriority("Coding"));
    expect(statePriority("Coding")).toBeLessThan(statePriority("Idle"));
    expect(statePriority("Idle", true)).toBeGreaterThan(statePriority("Idle"));
    expect(isBusy("Coding")).toBe(true);
    expect(isBusy("Waiting")).toBe(false);
  });

  it("designer preview cycles through all states", () => {
    const seen = new Set(Array.from({ length: ROBOT_STATES.length }, (_, i) => previewStateAt(i * 2600 + 10)));
    expect(seen.size).toBe(ROBOT_STATES.length);
    expect(previewStateAt(-5)).toBe("Idle");
  });
});

describe("robot poses", () => {
  it("each state has its own prop and is deterministic", () => {
    const props = ROBOT_STATES.map((s) => poseAt(s, 1234).prop);
    expect(new Set(props.filter((p) => p !== "none")).size).toBe(ROBOT_STATES.length - 1);
    expect(poseAt("Coding", 500)).toEqual(poseAt("Coding", 500));
    expect(poseAt("Completed", 0).eyes).toBe("happy");
    expect(poseAt("Error", 0).eyes).toBe("x");
    expect(poseAt("Idle", 0, 1, true).prop).toBe("sleep");
  });

  it("intensity 0 freezes the motion but keeps the state readable", () => {
    const a = poseAt("Completed", 100, 0);
    const b = poseAt("Completed", 700, 0);
    expect(Math.abs(a.bodyY)).toBe(0);
    expect(Math.abs(b.bodyY)).toBe(0);
    expect(a.prop).toBe("confetti");
  });

  it("blinks briefly", () => {
    expect(blink(1000)).toBe(1);
    expect(blink(70)).toBe(0);
  });
});

describe("performance tiers (§31, §70)", () => {
  const base = { mode: "UltraVisual" as const, reducedMotion: false, state: "Coding" as const, asleep: false, hovered: false, intensity: 1, hidden: false };
  it("caps fps per mode and goes static in Battery Saver / reduced motion / hidden", () => {
    expect(robotFps(base)).toBe(60);
    expect(robotFps({ ...base, mode: "Balanced" })).toBe(30);
    expect(robotFps({ ...base, mode: "Performance" })).toBe(15);
    expect(robotFps({ ...base, mode: "BatterySaver" })).toBe(0);
    expect(robotFps({ ...base, reducedMotion: true })).toBe(0);
    expect(robotFps({ ...base, hidden: true })).toBe(0);
    expect(robotFps({ ...base, intensity: 0 })).toBe(0);
  });
  it("idle robots rest unless hovered", () => {
    expect(robotFps({ ...base, state: "Idle" })).toBe(12);
    expect(robotFps({ ...base, state: "Idle", mode: "Performance" })).toBe(0);
    expect(robotFps({ ...base, state: "Idle", hovered: true })).toBe(60);
  });
  it("3D and glow only in the rich modes", () => {
    expect(robotEffects("UltraVisual", false)).toEqual({ glow: true, particles: true, threeD: true });
    expect(robotEffects("Balanced", false).threeD).toBe(false);
    expect(robotEffects("Performance", false).glow).toBe(false);
    expect(robotEffects("UltraVisual", true).threeD).toBe(false);
  });
  it("frame limiter", () => {
    expect(shouldRender(16, 0, 60)).toBe(true);
    expect(shouldRender(10, 0, 60)).toBe(false);
    expect(shouldRender(100, 0, 0)).toBe(false);
  });
});

describe("visibility (ADR-0016)", () => {
  const ids = ["a1", "a2", "a3", "a4"].map((s) => `0190f0b0-0000-7000-8000-0000000000${s}`);
  const [A, B, C, D] = ids as [string, string, string, string];
  const opts = { enabled: true, maxVisible: 2, lingerMs: 60_000, hidden: new Set<string>() };

  function spawned(): RobotSlots {
    let s: RobotSlots = new Map();
    for (const id of ids) s = applyRobotEvent(s, ev("AgentSpawned", { agent: agent(id) }, id), 0);
    return s;
  }

  it("spawns robots for AgentSpawned and follows AgentStateChanged", () => {
    let s = spawned();
    expect(s.size).toBe(4);
    s = applyRobotEvent(s, stateChanged(A, "Thinking", "Testing"), 10);
    expect(s.get(A)?.state).toBe("Testing");
    const same = applyRobotEvent(s, stateChanged(A, "Testing", "Testing"), 11);
    expect(same).toBe(s);
    s = applyRobotEvent(s, stateChanged(A, "Testing", "Completed"), 20);
    expect(s.get(A)?.finishedAt).toBe(20);
    s = applyRobotEvent(s, stateChanged(A, "Completed", "Coding"), 30);
    expect(s.get(A)?.finishedAt).toBeNull();
  });

  it("ignores updates of old runs and unknown agents", () => {
    const s = spawned();
    const old = agent("0190f0b0-0000-7000-8000-0000000000ff", { state: "Completed", stoppedAt: "x" });
    expect(applyRobotEvent(s, ev("AgentUpdated", { agent: old }), 5)).toBe(s);
    expect(applyRobotEvent(s, stateChanged("nope", "Idle", "Coding"), 5)).toBe(s);
    expect(applyRobotEvent(s, ev("TaskCreated", {}), 5)).toBe(s);
  });

  it("AgentStopped starts the linger timer once", () => {
    let s = spawned();
    s = applyRobotEvent(s, ev("AgentStopped", { agentId: B, reason: "UserRequested", detail: null }, B), 100);
    expect(s.get(B)?.finishedAt).toBe(100);
    expect(applyRobotEvent(s, ev("AgentStopped", { agentId: B, reason: "UserRequested", detail: null }, B), 200)).toBe(s);
  });

  it("selects up to maxVisible by priority, the rest overflow", () => {
    let s = spawned();
    s = applyRobotEvent(s, stateChanged(D, "Thinking", "Error"), 1);
    s = applyRobotEvent(s, stateChanged(C, "Thinking", "Waiting"), 1);
    const sel = selectRobots(s, opts, 2);
    expect(sel.visible).toEqual([D, C]);
    expect(sel.overflow).toEqual([A, B]);
  });

  it("respects off, hidden, linger, floating and auto-hide idle", () => {
    let s = spawned();
    expect(selectRobots(s, { ...opts, enabled: false }, 0)).toEqual({ visible: [], overflow: [] });
    expect(selectRobots(s, { ...opts, maxVisible: 9, hidden: new Set([A]) }, 0).visible).not.toContain(A);
    s = applyRobotEvent(s, stateChanged(B, "Thinking", "Completed"), 1000);
    expect(selectRobots(s, { ...opts, maxVisible: 9 }, 30_000).visible).toContain(B);
    expect(selectRobots(s, { ...opts, maxVisible: 9 }, 61_001).visible).not.toContain(B);
    const design = { ...defaultDesign(), behaviour: { ...defaultDesign().behaviour, autoHideIdle: true } };
    const c = s.get(C) as RobotSlot;
    const s2 = new Map(s);
    s2.set(C, { ...c, state: "Idle", design });
    s2.set(D, { ...(s.get(D) as RobotSlot), floating: false });
    const vis = selectRobots(s2, { ...opts, maxVisible: 9 }, 2000).visible;
    expect(vis).not.toContain(C);
    expect(vis).not.toContain(D);
  });

  it("prunes expired robots and schedules the next expiry", () => {
    let s = spawned();
    s = applyRobotEvent(s, stateChanged(A, "Thinking", "Completed"), 1000);
    s = applyRobotEvent(s, stateChanged(B, "Thinking", "Error"), 5000);
    expect(nextExpiry(s, 60_000)).toBe(61_000);
    const pruned = pruneSlots(s, 62_000, 60_000);
    expect(pruned.has(A)).toBe(false);
    expect(pruned.has(B)).toBe(true);
    expect(pruneSlots(pruned, 62_000, 60_000)).toBe(pruned);
  });

  it("merges robots.list entries with designs", () => {
    const design = { ...defaultDesign(), skin: "ember" };
    const s = mergeRobotEntries(new Map(), [{ agent: agent(A), design, floating: false }], 0);
    expect(s.get(A)?.design?.skin).toBe("ember");
    expect(s.get(A)?.floating).toBe(false);
  });

  it("diffs open windows against the wanted set", () => {
    expect(diffWindows(new Set([A, B]), [B, C])).toEqual({ toOpen: [C], toClose: [A] });
  });
});

describe("window planning", () => {
  const cfg: RobotsConfig = { ...defaultRobotsConfig(), positions: { [TEMPLATE]: { monitor: "M1", x: 100, y: 200 }, cluster: { monitor: null, x: 5, y: 6 } } };
  const slot: RobotSlot = { agentId: "x1", projectId: PROJECT, templateId: TEMPLATE, state: "Coding", finishedAt: null, floating: true, design: null };

  it("uses the template's remembered position once, then default slots", () => {
    const first = robotOpenArgs(slot, cfg, 0, new Set());
    expect(first.args).toMatchObject({ agentId: "x1", x: 100, y: 200, slot: 0, snap: true });
    expect(first.window.positionKey).toBe(TEMPLATE);
    const second = robotOpenArgs({ ...slot, agentId: "x2" }, cfg, 1, new Set([TEMPLATE]));
    expect(second.args).toMatchObject({ x: null, y: null, slot: 1 });
    expect(clusterOpenArgs(cfg, 3)).toMatchObject({ agentId: null, x: 5, y: 6, slot: 3 });
  });

  it("recreates a window when the design scale changes", () => {
    const big = { ...slot, design: { ...defaultDesign(), scale: 1.4 } };
    expect(staleWindows(new Map([["x1", { scale: 1, positionKey: null }]]), new Map([["x1", big]]), cfg)).toEqual(["x1"]);
    expect(staleWindows(new Map([["x1", { scale: 1.4, positionKey: null }]]), new Map([["x1", big]]), cfg)).toEqual([]);
  });
});

describe("speech bubbles", () => {
  const id = "0190f0b0-0000-7000-8000-0000000000b1";
  it("Scenario H: completion and test results produce result bubbles", () => {
    expect(bubbleForEvent(stateChanged(id, "Testing", "Completed"))).toMatchObject({ key: "completed", tone: "success", important: true });
    const passed = bubbleForEvent(ev("TestRunFinished", { runId: "r", passed: 42, failed: 0, skipped: 0, durationMs: 1, summary: "" }, id));
    expect(passed).toMatchObject({ key: "testsPassed", values: { count: 42 }, tone: "success" });
    const failed = bubbleForEvent(ev("TestRunFinished", { runId: "r", passed: 1, failed: 3, skipped: 0, durationMs: 1, summary: "" }, id));
    expect(failed).toMatchObject({ key: "testsFailed", values: { count: 3 }, tone: "danger" });
    expect(bubbleForEvent(stateChanged(id, "Coding", "Error", "cargo failed"))).toMatchObject({ key: "errorDetail", values: { detail: "cargo failed" } });
  });

  it("a robot opened on a finished agent still announces the result", () => {
    expect(initialBubble("Completed", null, "Normal")).toMatchObject({ key: "completed" });
    expect(initialBubble("Error", "boom", "Minimal")).toMatchObject({ key: "errorDetail", values: { detail: "boom" } });
    expect(initialBubble("Coding", null, "Chatty")).toBeNull();
    expect(initialBubble("Completed", null, "Off")).toBeNull();
  });

  it("filters by verbosity", () => {
    const thinking = bubbleForEvent(stateChanged(id, "Idle", "Thinking"));
    const done = bubbleForEvent(stateChanged(id, "Testing", "Completed"));
    const testing = bubbleForEvent(stateChanged(id, "Coding", "Testing"));
    if (!thinking || !done || !testing) throw new Error("bubbles expected");
    expect(passesVerbosity(done, "Off")).toBe(false);
    expect(passesVerbosity(done, "Minimal")).toBe(true);
    expect(passesVerbosity(testing, "Minimal")).toBe(false);
    expect(passesVerbosity(testing, "Normal")).toBe(true);
    expect(passesVerbosity(thinking, "Normal")).toBe(false);
    expect(passesVerbosity(thinking, "Chatty")).toBe(true);
    expect(bubbleFor(ev("Log", {}), "Chatty")).toBeNull();
  });

  it("fresh important bubbles are not replaced by chatter", () => {
    const done = bubbleForEvent(stateChanged(id, "Testing", "Completed"));
    const testing = bubbleForEvent(stateChanged(id, "Coding", "Testing"));
    if (!done || !testing) throw new Error("bubbles expected");
    expect(mayReplace(null, testing, 0)).toBe(true);
    expect(mayReplace({ spec: done, shownAt: 0 }, testing, 1000)).toBe(false);
    expect(mayReplace({ spec: done, shownAt: 0 }, testing, 4000)).toBe(true);
    expect(mayReplace({ spec: testing, shownAt: 0 }, done, 10)).toBe(true);
  });
});

describe("hit-testing and layout (RR-C4)", () => {
  it("scales the window with the design and keeps the robot inside", () => {
    for (const scale of [0.6, 1, 1.6, 9]) {
      const l = robotWindowLayout(scale);
      expect(l.robot.x).toBeGreaterThanOrEqual(0);
      expect(l.robot.x + l.robot.w).toBeLessThanOrEqual(l.width);
      expect(l.robot.y + l.robot.h).toBeLessThanOrEqual(l.height);
    }
    expect(robotWindowLayout(1.6).height).toBeGreaterThan(robotWindowLayout(1).height);
  });

  it("only the robot body (and bubble / menu) is interactive", () => {
    const layout = robotWindowLayout(1);
    const shape = hitShapeFor({ layout, bubble: null, menuOpen: false });
    const cx = layout.robot.x + layout.robot.w / 2;
    const cy = layout.robot.y + layout.robot.h / 2;
    expect(hitTest(shape, cx, cy)).toBe(true);
    expect(hitTest(shape, 1, 1)).toBe(false);
    expect(hitTest(shape, layout.robot.x + 1, layout.robot.y + 1)).toBe(false);
    const withBubble = hitShapeFor({ layout, bubble: { x: 10, y: 10, w: 100, h: 40 }, menuOpen: false });
    expect(hitTest(withBubble, 50, 30)).toBe(true);
    const menu = hitShapeFor({ layout, bubble: null, menuOpen: true });
    expect(hitTest(menu, 1, 1)).toBe(true);
  });

  it("matches the Rust ellipse test at the boundary", () => {
    const shape = { ellipses: [{ cx: 100, cy: 200, rx: 60, ry: 70 }], rects: [] };
    expect(hitTest(shape, 159, 200)).toBe(true);
    expect(hitTest(shape, 161, 200)).toBe(false);
    expect(hitTest({ ellipses: [{ cx: 0, cy: 0, rx: 0, ry: 5 }], rects: [] }, 0, 0)).toBe(false);
  });

  it("tells drags from clicks", () => {
    expect(isDragGesture(1, 2)).toBe(false);
    expect(isDragGesture(3, 3)).toBe(true);
  });

  it("collects drop zones from project tiles", () => {
    document.body.innerHTML = `<button data-robot-drop="p1"></button><button data-robot-drop=""></button><div></div>`;
    const el = document.querySelector<HTMLElement>("[data-robot-drop='p1']");
    if (!el) throw new Error("tile");
    el.getBoundingClientRect = () => ({ left: 8, top: 60, width: 44, height: 44, right: 52, bottom: 104, x: 8, y: 60, toJSON: () => ({}) });
    expect(collectDropZones(document)).toEqual([{ id: "p1", rect: { x: 8, y: 60, w: 44, h: 44 } }]);
  });
});

describe("robot design schema", () => {
  it("default design is valid and mirrors the Rust defaults", () => {
    expect(validateDesign(defaultDesign())).toEqual([]);
    expect(defaultRobotsConfig()).toMatchObject({ schemaVersion: 1, enabled: true, maxVisible: 6, lingerSeconds: 90 });
  });

  it("reports the same paths as the core validator", () => {
    const bad = { ...defaultDesign(), skin: "plaid", accent: "red", scale: 9, animationIntensity: -1, accessories: ["laser"], sounds: "boom", voice: { enabled: true, voiceId: "" } };
    const codes = validateDesign(bad, "defaultDesign").map((i) => i.path);
    expect(codes).toEqual(
      expect.arrayContaining([
        "defaultDesign.skin",
        "defaultDesign.accent",
        "defaultDesign.scale",
        "defaultDesign.animationIntensity",
        "defaultDesign.accessories.0",
        "defaultDesign.sounds",
        "defaultDesign.voice.voiceId",
      ]),
    );
  });

  it("normalizes untrusted input into a valid design", () => {
    const d = normalizeDesign({ skin: "plaid", accent: "#ABCDEF", scale: 99, accessories: ["halo", "halo", "laser", "scarf", "visor", "bowtie", "jetpack"] });
    expect(validateDesign(d)).toEqual([]);
    expect(d.skin).toBe("nebula");
    expect(d.accent).toBe("#abcdef");
    expect(d.scale).toBe(1.6);
    expect(d.accessories).toEqual(["halo", "scarf", "visor", "bowtie"]);
    expect(normalizeDesign(null)).toEqual(defaultDesign());
  });

  it("accessory toggling respects the cap; accent falls back to the skin", () => {
    expect(toggleAccessory(["antenna"], "antenna")).toEqual([]);
    expect(toggleAccessory(["antenna", "halo", "scarf", "visor"], "bowtie")).toHaveLength(4);
    expect(accentOf({ skin: "ember", accent: null })).toBe("#ffb347");
    expect(accentOf({ skin: "ember", accent: "#123456" })).toBe("#123456");
  });
});

describe("routes, menu, assign", () => {
  it("parses robot window routes defensively", () => {
    expect(parseRobotRoute("#/agent/0190f0b0-0000-7000-8000-000000000001")).toEqual({ kind: "agent", agentId: "0190f0b0-0000-7000-8000-000000000001" });
    expect(parseRobotRoute("#/cluster")).toEqual({ kind: "cluster" });
    expect(parseRobotRoute("#/preview")).toEqual({ kind: "preview" });
    expect(parseRobotRoute("#/agent/../evil")).toEqual({ kind: "invalid" });
    expect(parseRobotRoute("")).toEqual({ kind: "invalid" });
  });

  it("always offers Stop agent while the agent runs", () => {
    const running = robotMenu({ state: "Coding", stoppedAt: null, voiceEnabled: false, assignableProjects: 2, hasTemplate: true });
    expect(running.find((i) => i.id === "stop")?.enabled).toBe(true);
    expect(running.find((i) => i.id === "assign")?.enabled).toBe(true);
    const done = robotMenu({ state: "Completed", stoppedAt: "t", voiceEnabled: true, assignableProjects: 0, hasTemplate: false });
    expect(done.find((i) => i.id === "stop")?.enabled).toBe(false);
    expect(done.find((i) => i.id === "assign")?.enabled).toBe(false);
    expect(done.find((i) => i.id === "voice")?.checked).toBe(true);
    expect(isRunning("Waiting", null)).toBe(true);
    expect(nextEnabled(done, 0, 1)).toBe(1 + done.slice(1).findIndex((i) => i.enabled));
    expect(nextEnabled([], 0, 1)).toBe(-1);
  });

  it("plans the assign of a dropped robot", () => {
    const projects = [PROJECT, OTHER_PROJECT];
    expect(planAssign({ agent: { projectId: PROJECT, templateId: TEMPLATE }, projectId: OTHER_PROJECT, projectIds: projects })).toEqual({
      kind: "spawn",
      templateId: TEMPLATE,
      projectId: OTHER_PROJECT,
    });
    expect(planAssign({ agent: { projectId: PROJECT, templateId: TEMPLATE }, projectId: PROJECT, projectIds: projects }).kind).toBe("sameProject");
    expect(planAssign({ agent: { projectId: PROJECT, templateId: null }, projectId: OTHER_PROJECT, projectIds: projects }).kind).toBe("noTemplate");
    expect(planAssign({ agent: null, projectId: PROJECT, projectIds: projects }).kind).toBe("unknownAgent");
    expect(planAssign({ agent: { projectId: PROJECT, templateId: TEMPLATE }, projectId: "zzz", projectIds: projects }).kind).toBe("unknownProject");
  });
});
