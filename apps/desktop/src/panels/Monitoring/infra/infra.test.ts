import type { LogLine, MetricSeries, NodeMetricsSnapshot } from "@orkestr/protocol";
import { describe, expect, it } from "vitest";

import {
  type CheckStatusView,
  checkTone,
  draftFromCheck,
  draftToParams,
  EMPTY_DRAFT,
  lastTs,
  mergeTail,
  probeSummary,
  seriesValues,
  snapshotPercents,
  upRatio,
} from "./model";

const line = (ts: string, message: string, sourceId = "ingest:web"): LogLine => ({ ts, sourceId, message });

describe("infra model", () => {
  it("computes node percentages safely", () => {
    const n: NodeMetricsSnapshot = {
      nodeId: "n",
      name: "local",
      local: true,
      online: true,
      cpuPercent: 140,
      memUsedBytes: 25,
      memTotalBytes: 100,
      gpuPercent: [10, 70],
      diskUsedBytes: 5,
      diskTotalBytes: 0,
      sampledAt: "2026-10-22T10:00:00Z",
    };
    expect(snapshotPercents(n)).toEqual({ cpu: 100, mem: 25, disk: 0, gpu: 70 });
  });

  it("extracts series values", () => {
    const s: MetricSeries[] = [{ nodeId: "n", metric: "cpu", unit: "percent", points: [{ ts: "t", value: 3 }] }];
    expect(seriesValues(s, "cpu")).toEqual([3]);
    expect(seriesValues(s, "mem")).toEqual([]);
    expect(seriesValues(null, "cpu")).toEqual([]);
  });

  it("validates check drafts and builds params", () => {
    expect(draftToParams({ ...EMPTY_DRAFT })).toEqual({ error: "name" });
    expect(draftToParams({ ...EMPTY_DRAFT, name: "x", url: "ftp://a" })).toEqual({ error: "url" });
    expect(draftToParams({ ...EMPTY_DRAFT, name: "x", url: "http://a", expectStatus: "99" })).toEqual({ error: "status" });
    expect(draftToParams({ ...EMPTY_DRAFT, name: "x", kind: "Tcp", host: "db", port: "70000" })).toEqual({ error: "port" });
    expect(draftToParams({ ...EMPTY_DRAFT, name: "x", url: "http://a", intervalSecs: "-1" })).toEqual({ error: "numbers" });
    const ok = draftToParams({ ...EMPTY_DRAFT, name: " api ", url: "https://a/health", method: "head", expectStatus: "204", contains: "ok" });
    expect(ok).toEqual({
      params: {
        name: "api",
        probe: { type: "Http", url: "https://a/health", method: "HEAD", expectStatus: 204, contains: "ok" },
        intervalSecs: 30,
        timeoutMs: 5000,
        failureThreshold: 2,
        enabled: true,
      },
    });
    const tcp = draftToParams({ ...EMPTY_DRAFT, id: "c1", name: "db", kind: "Tcp", host: "db", port: "5432" });
    expect("params" in tcp && tcp.params.id).toBe("c1");
    expect("params" in tcp && probeSummary(tcp.params.probe)).toBe("tcp db:5432");
  });

  it("round-trips a check into a draft", () => {
    const c: CheckStatusView = {
      check: {
        id: "c",
        name: "api",
        probe: { type: "Http", url: "http://x", expectStatus: 200 },
        intervalSecs: 10,
        timeoutMs: 1000,
        failureThreshold: 3,
        enabled: false,
      },
      state: "Down",
      history: [
        { ts: "a", ok: true },
        { ts: "b", ok: false },
      ],
    };
    const d = draftFromCheck(c);
    expect(d).toMatchObject({ id: "c", kind: "Http", method: "GET", expectStatus: "200", intervalSecs: "10", enabled: false });
    expect(upRatio(c.history)).toBe(0.5);
    expect(upRatio([])).toBeNull();
    expect(checkTone("Down")).toBe("danger");
    expect(probeSummary(c.check.probe)).toBe("GET http://x");
  });

  it("merges polled tail lines without duplicates and caps the buffer", () => {
    const prev = [line("2026-01-01T00:00:01Z", "a"), line("2026-01-01T00:00:02Z", "b")];
    const merged = mergeTail(prev, [line("2026-01-01T00:00:02Z", "b"), line("2026-01-01T00:00:03Z", "c")]);
    expect(merged.map((l) => l.message)).toEqual(["a", "b", "c"]);
    expect(lastTs(merged)).toBe("2026-01-01T00:00:03Z");
    expect(mergeTail(merged, [line("2026-01-01T00:00:04Z", "d")], 2).map((l) => l.message)).toEqual(["c", "d"]);
    expect(mergeTail(prev, [])).toBe(prev);
  });
});
