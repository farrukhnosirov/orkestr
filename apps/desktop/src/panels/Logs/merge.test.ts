import type { Event } from "@orkestr/protocol";
import { describe, expect, it } from "vitest";

import { mergeEvents } from "./merge";

function ev(seq: number, ts: string, id = `e${seq}-${ts}`): Event {
  return {
    seq,
    id,
    ts,
    kind: "Log",
    payload: { level: "info", target: "test", message: id },
  } as unknown as Event;
}

const seqs = (list: Event[]) => list.map((e) => e.seq);

describe("mergeEvents", () => {
  it("puts history before live events that arrived first (regression)", () => {
    // Live events #948..#952 arrive while events.replay is in flight…
    let list: Event[] = [];
    for (let s = 948; s <= 952; s++) list = mergeEvents(list, [ev(s, `2026-10-07T13:45:0${s - 948}Z`)], 1000);
    // …then the ascending history page arrives, overlapping #948..#950.
    const history = [];
    for (let s = 900; s <= 950; s++) history.push(ev(s, "2026-10-07T13:36:00Z", `e${s}-${s >= 948 ? `2026-10-07T13:45:0${s - 948}Z` : "2026-10-07T13:36:00Z"}`));
    list = mergeEvents(list, history, 1000);
    const out = seqs(list);
    expect(out).toEqual([...out].sort((a, b) => a - b));
    expect(out[0]).toBe(900);
    expect(out.at(-1)).toBe(952);
    expect(new Set(out).size).toBe(out.length);
  });

  it("dedupes by id and keeps only the newest `max` events", () => {
    const list = mergeEvents([ev(1, "2026-10-07T00:00:01Z"), ev(2, "2026-10-07T00:00:02Z")], [ev(3, "2026-10-07T00:00:03Z"), ev(2, "2026-10-07T00:00:02Z")], 2);
    expect(seqs(list)).toEqual([2, 3]);
  });

  it("places local seq=0 events by timestamp, stably", () => {
    const a = ev(0, "2026-10-07T00:00:02.500Z", "local-a");
    const b = ev(0, "2026-10-07T00:00:02.500Z", "local-b");
    const list = mergeEvents(
      [ev(1, "2026-10-07T00:00:01Z"), ev(3, "2026-10-07T00:00:03Z")],
      [a, b, ev(2, "2026-10-07T00:00:02Z")],
      100,
    );
    expect(list.map((e) => e.id)).toEqual([
      "e1-2026-10-07T00:00:01Z",
      "e2-2026-10-07T00:00:02Z",
      "local-a",
      "local-b",
      "e3-2026-10-07T00:00:03Z",
    ]);
  });

  it("returns the same list when nothing arrives", () => {
    const prev = [ev(1, "2026-10-07T00:00:01Z")];
    expect(mergeEvents(prev, [], 10)).toBe(prev);
  });
});
