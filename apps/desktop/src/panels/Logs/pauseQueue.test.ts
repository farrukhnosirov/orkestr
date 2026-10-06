import { describe, expect, it } from "vitest";

import { PauseQueue } from "./pauseQueue";

describe("PauseQueue", () => {
  it("keeps only the newest `max` items but counts everything received", () => {
    const q = new PauseQueue<number>(3);
    for (let i = 1; i <= 10_000; i++) q.push(i);
    expect(q.size).toBe(3);
    expect(q.count).toBe(10_000);
    expect(q.drain()).toEqual([9_998, 9_999, 10_000]);
    expect(q.size).toBe(0);
    expect(q.count).toBe(0);
  });

  it("returns everything while under the cap", () => {
    const q = new PauseQueue<string>(5);
    q.push("a");
    q.push("b");
    expect(q.drain()).toEqual(["a", "b"]);
  });
});
