import type { Event, OrkestrProcess, ResourceSample } from "@orkestr/protocol";
import { describe, expect, it } from "vitest";

import { linePoints, loadTone, pathOf } from "./charts";
import { latencyStats, levelOf, logLineOf, mergeTail, sortProcesses, worstLevel } from "./health";
import { clampPct, MAX_POINTS, pointFromSample, pushPoint, ratio, seriesStats, type TelemetryPoint, windowed } from "./telemetry";

function sample(patch: Partial<ResourceSample> = {}): ResourceSample {
  return {
    nodeId: "n1",
    ts: "2026-10-05T10:00:00Z",
    cpuPercent: 42,
    perCorePercent: [10, 120, -5],
    memoryTotalBytes: 1000,
    memoryUsedBytes: 250,
    swapTotalBytes: 0,
    swapUsedBytes: 0,
    diskTotalBytes: 500,
    diskFreeBytes: 100,
    coreProcess: { pid: 7, name: "orkestr", cpuPercent: 3, cpuCores: 0.1, memoryBytes: 64 },
    topProcesses: [],
    ...patch,
  };
}

const pt = (ts: number, cpu = 0): TelemetryPoint => ({ ...pointFromSample(sample()), ts, cpu });

describe("telemetry", () => {
  it("converts samples and clamps percentages", () => {
    const p = pointFromSample(sample());
    expect(p.ts).toBe(Date.parse("2026-10-05T10:00:00Z"));
    expect(p.perCore).toEqual([10, 100, 0]);
    expect(p.diskUsed).toBe(400);
    expect(p.coreCpu).toBe(3);
    expect(p.gpu).toBeNull();
    expect(clampPct(Number.NaN)).toBe(0);
  });

  it("appends in order, drops stale samples and caps the buffer", () => {
    let buf: TelemetryPoint[] = [];
    buf = pushPoint(buf, pt(1));
    buf = pushPoint(buf, pt(2));
    expect(pushPoint(buf, pt(2))).toBe(buf);
    expect(pushPoint(buf, pt(1))).toBe(buf);
    let big: TelemetryPoint[] = [];
    for (let i = 1; i <= MAX_POINTS + 5; i++) big = pushPoint(big, pt(i));
    expect(big).toHaveLength(MAX_POINTS);
    expect(big[0]?.ts).toBe(6);
  });

  it("windows and summarises series", () => {
    const pts = [pt(1000, 10), pt(2000, 30), pt(3000, 20)];
    expect(windowed(pts, 1500, 3000).map((p) => p.ts)).toEqual([2000, 3000]);
    expect(seriesStats([10, 30, 20])).toEqual({ min: 10, max: 30, avg: 20, last: 20 });
    expect(seriesStats([])).toBeNull();
    expect(ratio(5, 0)).toBe(0);
    expect(ratio(15, 10)).toBe(1);
  });
});

describe("charts", () => {
  it("maps values into the box (top = max)", () => {
    const pts = linePoints([0, 50, 100, 200], 30, 20, 100, 0);
    expect(pts).toEqual([
      [0, 20],
      [10, 10],
      [20, 0],
      [30, 0],
    ]);
    expect(pathOf(pts.slice(0, 2))).toBe("M0.0,20.0L10.0,10.0");
    expect(linePoints([], 10, 10, 1)).toEqual([]);
    expect(linePoints([5], 10, 10, 10, 0)).toEqual([[10, 5]]);
  });

  it("escalates tones at 75 % and 90 %", () => {
    expect(loadTone(0.5, "violet")).toBe("violet");
    expect(loadTone(0.8)).toBe("warning");
    expect(loadTone(0.95)).toBe("danger");
    expect(loadTone(null, "info")).toBe("info");
  });
});

describe("health", () => {
  it("evaluates thresholds", () => {
    expect(levelOf(10, 100, 200)).toBe("ok");
    expect(levelOf(150, 100, 200)).toBe("warn");
    expect(levelOf(250, 100, 200)).toBe("fail");
    expect(levelOf(null, 100)).toBe("unknown");
    expect(worstLevel(["ok", "unknown", "warn"])).toBe("warn");
    expect(worstLevel(["unknown"])).toBe("unknown");
    expect(worstLevel([])).toBe("unknown");
  });

  it("computes latency statistics", () => {
    const s = latencyStats([10, 20, 30, 40, 1000, Number.NaN]);
    expect(s).toMatchObject({ last: 1000, max: 1000, samples: 5 });
    expect(s?.avg).toBeCloseTo(220);
    expect(s?.p95).toBe(1000);
    expect(latencyStats([])).toBeNull();
  });

  it("keeps warn/error log lines, newest first, deduplicated", () => {
    const log = (id: string, seq: number, level: string): Event =>
      ({
        id,
        seq,
        ts: "2026-10-05T10:00:00Z",
        projectId: null,
        taskId: null,
        agentId: null,
        nodeId: null,
        correlationId: null,
        kind: "Log",
        payload: { level, target: "core::rpc", message: `m${id}`, fields: null },
      }) as Event;
    expect(logLineOf(log("i", 1, "info"))).toBeNull();
    const lines = [log("a", 1, "warn"), log("b", 3, "error"), log("c", 2, "debug")]
      .map(logLineOf)
      .filter((l) => l !== null);
    const tail = mergeTail([], lines);
    expect(tail.map((l) => l.id)).toEqual(["b", "a"]);
    expect(mergeTail(tail, lines).map((l) => l.id)).toEqual(["b", "a"]);
    expect(mergeTail(tail, [], 1)).toBe(tail);
  });

  it("sorts processes: running first, then by the chosen key", () => {
    const p = (id: string, running: boolean, cpu: number | null, mem: number | null): OrkestrProcess => ({
      kind: "Terminal",
      id,
      label: id,
      projectId: null,
      agentId: null,
      pid: 1,
      running,
      cpuPercent: cpu,
      memoryBytes: mem,
      processCount: 1,
    });
    const rows = [p("z", false, 99, 99), p("b", true, 5, 500), p("a", true, 50, 100), p("c", true, null, null)];
    expect(sortProcesses(rows, "cpu").map((r) => r.id)).toEqual(["a", "b", "c", "z"]);
    expect(sortProcesses(rows, "memory").map((r) => r.id)).toEqual(["b", "a", "c", "z"]);
    expect(sortProcesses(rows, "name").map((r) => r.id)).toEqual(["a", "b", "c", "z"]);
  });
});
