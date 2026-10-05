/**
 * Phase 3 integration seams (Visual OS ↔ Phase 2 quality data, theme ↔
 * sound / robots, performance policy, layout widget state, AI-editable UI).
 */
import type { CheckResult, Event, QualityRun, Review, ThemeSounds } from "@orkestr/protocol";
import { validatePrefs, validateUiConfig, defaultUiConfig } from "@orkestr/ui";
import { describe, expect, it } from "vitest";

import { layoutIdForPreset } from "./features/aiui/useAiUiRuntime";
import { validateLayoutDocument } from "./features/layout/schema";
import { resolvePolicy } from "./features/performance/policy";
import { buildErrorsOf, hudStateOf, mergeMissionRows, missionRowsOf, openReviewCount, qualityFactsOf } from "./features/quality/signals";
import { bubbleForEvent } from "./features/robots/model/bubble";
import { defaultDesign, withThemeRobotDefaults } from "./features/robots/model/design";
import { FPS_BY_MODE, robotEffects } from "./features/robots/model/perf";
import { pipelineStateFor, withPipelineState } from "./features/robots/model/states";
import { THEME_SOUND_EVENTS, THEME_SOUND_PRESETS, themeCueOverride } from "./features/sound/recipes";
import { ROBOT_SKINS, SOUND_EVENTS, SOUND_PRESETS } from "./features/theme/logic";
import { widgetPrefIds } from "./panels/MissionControl/Dashboard";

function check(over: Partial<CheckResult>): CheckResult {
  return {
    checkId: "c1",
    kind: "Build",
    toolchain: "Node",
    label: "npm run build",
    command: "npm run build",
    cwd: ".",
    status: "Passed",
    durationMs: 1200,
    passed: 0,
    failed: 0,
    skipped: 0,
    failures: [],
    diagnostics: [],
    rawTail: "",
    summary: "ok",
    ...over,
  } as CheckResult;
}

function run(id: string, startedAt: string, results: CheckResult[]): QualityRun {
  return {
    id,
    projectId: "p1",
    target: { type: "project" },
    workspace: "/w",
    kinds: results.map((r) => r.kind),
    status: "Passed",
    results,
    startedAt,
    finishedAt: startedAt,
    durationMs: 10,
  } as QualityRun;
}

function ev(kind: string, payload: unknown): Event {
  return { seq: 1, id: "e1", ts: "2026-10-05T10:00:00Z", projectId: "p1", taskId: "t1", agentId: null, nodeId: null, correlationId: null, kind, payload } as unknown as Event;
}

describe("Phase 2 quality signals", () => {
  const runs = [
    run("r2", "2026-10-05T10:00:00Z", [
      check({ checkId: "build", status: "Failed", summary: "tsc failed", exitCode: 2, diagnostics: [{ severity: "Error", message: "TS2304: x", file: "src/a.ts", line: 3 }] as CheckResult["diagnostics"] }),
      check({ checkId: "test", kind: "Test", status: "Failed", passed: 8, failed: 2 }),
    ]),
    run("r1", "2026-10-05T09:00:00Z", [check({ checkId: "build" }), check({ checkId: "lint", kind: "Lint" })]),
  ];

  it("maps runner checks to Mission Control rows with event-compatible ids", () => {
    const rows = missionRowsOf(runs);
    expect(rows.map((r) => r.id)).toEqual(["r2:build", "r2:test", "r1:build"]);
    expect(rows[1]).toMatchObject({ kind: "test", status: "failed", passed: 8, failed: 2 });
    const live = [{ ...rows[0]!, status: "passed" as const }];
    expect(mergeMissionRows(live, rows)[0]?.status).toBe("passed");
  });

  it("summarises the latest build / tests for the HUD and voice", () => {
    const facts = qualityFactsOf(runs);
    expect(facts.lastBuild).toMatchObject({ ok: false, summary: "tsc failed" });
    expect(facts.lastTests).toMatchObject({ passed: 8, failed: 2 });
    expect(hudStateOf(facts)).toBe("failing");
    expect(hudStateOf(qualityFactsOf([]))).toBeNull();
    expect(buildErrorsOf(runs)[0]).toMatchObject({ summary: "src/a.ts:3: TS2304: x", exitCode: 2 });
  });

  it("counts open reviews", () => {
    const review = (status: Review["status"], verdict: Review["verdict"], open: boolean) =>
      ({ status, verdict, findings: open ? [{ status: "Open" }] : [] }) as unknown as Review;
    expect(openReviewCount([review("Running", null, false), review("Finished", "RequestChanges", true), review("Finished", "Approve", true)])).toBe(2);
  });
});

describe("robots follow the quality pipeline and the theme", () => {
  it("pipeline stages drive Testing / Reviewing", () => {
    const stage = (stage: string, status: string) => ev("PipelineStageChanged", { pipelineId: "x", projectId: "p1", taskId: "t1", stage, status, runs: 1 });
    expect(pipelineStateFor(stage("Test", "Running"))).toBe("Testing");
    expect(pipelineStateFor(stage("Security", "Running"))).toBe("Reviewing");
    expect(pipelineStateFor(stage("Test", "Passed"))).toBeNull();
    expect(pipelineStateFor(ev("TaskUpdated", {}))).toBeUndefined();
    expect(withPipelineState({ state: "Coding", asleep: false }, "Testing").state).toBe("Testing");
    expect(withPipelineState({ state: "Error", asleep: false }, "Testing").state).toBe("Error");
    expect(bubbleForEvent(stage("Build", "Failed"))?.key).toBe("buildFailed");
  });

  it("adversarial review findings become a bubble", () => {
    const review = { kind: "Adversarial", status: "Finished", verdict: "RequestChanges", findings: [{}, {}] };
    expect(bubbleForEvent(ev("ReviewUpdated", { review }))).toMatchObject({ key: "securityFindings", values: { count: 2 } });
  });

  it("theme robot defaults style unstyled robots only", () => {
    const d = defaultDesign();
    expect(withThemeRobotDefaults(d, { skin: "ember", accent: "#f59e0b" })).toMatchObject({ skin: "ember", accent: "#f59e0b" });
    expect(withThemeRobotDefaults({ ...d, skin: "mint" }, { skin: "ember", accent: null }).skin).toBe("mint");
    expect(withThemeRobotDefaults(d, { skin: "not-a-skin", accent: null })).toBe(d);
    expect(ROBOT_SKINS).toContain("ember");
  });

  it("robot frame rates and 3D come from the central performance table", () => {
    expect(FPS_BY_MODE.UltraVisual).toBe(resolvePolicy("UltraVisual", { reducedMotion: false, hidden: false }).robotFps);
    expect(FPS_BY_MODE.BatterySaver).toBe(0);
    expect(robotEffects("UltraVisual", false).threeD).toBe(true);
    expect(robotEffects("Balanced", false).threeD).toBe(false);
  });
});

describe("theme sounds drive the sound engine", () => {
  const sounds: ThemeSounds = { enabled: true, volume: 0.5, events: { "task.completed": "chime-soft", "agent.error": "none" } };
  it("the editor catalogue is the engine catalogue", () => {
    expect(SOUND_EVENTS).toEqual(Object.keys(THEME_SOUND_EVENTS));
    expect(SOUND_PRESETS).toEqual(Object.keys(THEME_SOUND_PRESETS));
  });
  it("maps, mutes and scales cues", () => {
    expect(themeCueOverride("completion", sounds)).toEqual({ mute: false, preset: "Crystal", volume: 0.5 });
    expect(themeCueOverride("error", sounds).mute).toBe(true);
    expect(themeCueOverride("wake", sounds)).toEqual({ mute: false, preset: null, volume: 0.5 });
    expect(themeCueOverride("wake", { ...sounds, enabled: false }).mute).toBe(true);
    expect(themeCueOverride("wake", null)).toEqual({ mute: false, preset: null, volume: 1 });
  });
});

describe("UI config schema v2 and AI-editable UI targets", () => {
  it("accepts v1 and v2 documents and validates prefs like Rust", () => {
    const v2 = defaultUiConfig();
    expect(validateUiConfig(v2).ok).toBe(true);
    expect(validateUiConfig({ ...v2, schemaVersion: 1 }).ok).toBe(true);
    expect(validateUiConfig({ ...v2, schemaVersion: 99 }).ok).toBe(false);
    expect(validatePrefs({ backgroundDim: 2, hiddenWidgets: ["Bad Id"] }).map((i) => i.path)).toEqual(["prefs.hiddenWidgets.0", "prefs.backgroundDim"]);
  });
  it("layout presets and Mission Control widgets are addressable", () => {
    expect(layoutIdForPreset("MissionControl")).toBe("mission-control");
    expect(layoutIdForPreset("Custom")).toBeNull();
    expect(widgetPrefIds("budgets")).toContain("missionControl.tokenChart");
    expect(widgetPrefIds("quality")).toEqual(["missionControl.quality"]);
  });
  it("layout documents carry bounded widget state", () => {
    const doc = {
      schemaVersion: 1,
      id: "coding",
      name: "Coding",
      dock: {},
      panels: ["code"],
      locked: false,
      windows: { mainMaximized: false, popouts: [] },
      widgets: { missionControl: "{}" },
    };
    expect(validateLayoutDocument(doc as never)).toEqual([]);
    expect(validateLayoutDocument({ ...doc, widgets: { "bad id": "x" } } as never).map((i) => i.code)).toContain("widgets");
  });
});
