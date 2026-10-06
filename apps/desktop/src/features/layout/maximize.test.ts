import type { DockviewApi } from "dockview-react";
import { describe, expect, it } from "vitest";

import { captureGroupSizes, restoreGroupSizes } from "./controller";

/**
 * A row of grid groups whose total width is fixed: resizing one group gives
 * the difference to (or takes it from) its right neighbour, like a split view.
 */
function fakeRow(widths: number[], floating: number[] = []) {
  const groups = widths.map((w, i) => ({
    id: `g${i}`,
    w,
    api: {
      location: { type: floating.includes(i) ? "floating" : "grid" },
      get width() {
        return groups[i]!.w;
      },
      height: 800,
      setSize({ width }: { width?: number }) {
        if (width === undefined) return;
        const next = groups[i + 1] ?? groups[i - 1];
        if (!next) return;
        next.w += groups[i]!.w - width;
        groups[i]!.w = width;
      },
    },
  }));
  return { dock: { groups } as unknown as DockviewApi, groups };
}

describe("maximize / restore keeps group sizes", () => {
  it("puts every grid group back on its recorded width", () => {
    const { dock, groups } = fakeRow([212, 329, 501, 293]);
    const before = captureGroupSizes(dock);
    // What dockview's exit-maximize left behind: the side column squeezed.
    groups[2]!.w = 696;
    groups[3]!.w = 98;
    restoreGroupSizes(dock, before);
    expect(groups.map((g) => g.w)).toEqual([212, 329, 501, 293]);
  });

  it("ignores floating groups and groups that are gone", () => {
    const { dock, groups } = fakeRow([300, 300, 400], [2]);
    const before = captureGroupSizes(dock);
    expect(before.map((s) => s.id)).toEqual(["g0", "g1"]);
    groups[0]!.w = 250;
    groups[1]!.w = 350;
    restoreGroupSizes(dock, [...before, { id: "missing", width: 10, height: 10 }]);
    expect(groups.map((g) => g.w)).toEqual([300, 300, 400]);
  });
});
