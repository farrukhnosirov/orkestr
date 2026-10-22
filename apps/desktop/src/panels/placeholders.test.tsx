/**
 * Regression: placeholder panels must announce their planned phase (not a
 * completed one) and built-in presets must never place a placeholder.
 */
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { BUILTIN_PRESETS } from "../features/layout/presets";
import BrowserPanel from "./Browser";
import { PLANNED_PHASE } from "./plannedPhases";
import { PANELS, getPanel } from "./registry";

afterEach(() => cleanup());

const CURRENT_PHASE = 3;

describe("placeholder panels", () => {
  it("are announced for a future phase that matches the plan of record", () => {
    for (const p of PANELS.filter((x) => x.phase > 0)) {
      expect(p.phase).toBeGreaterThan(CURRENT_PHASE);
      expect(PLANNED_PHASE[p.id as keyof typeof PLANNED_PHASE]).toBe(p.phase);
    }
    expect(getPanel("browser")?.phase).toBe(4);
    expect(getPanel("database")?.phase).toBe(4);
    expect(getPanel("deployments")?.phase).toBe(0);
  });

  it("the badge shows the planned phase", () => {
    render(<BrowserPanel projectId={null} />);
    expect(screen.getByText("Arrives in Phase 4")).toBeTruthy();
    expect(screen.queryByText("Arrives in Phase 2")).toBeNull();
  });

  it("are never placed by a built-in preset (slots, compact slots or monitor spread)", () => {
    for (const preset of BUILTIN_PRESETS) {
      const ids = [
        ...preset.slots.map((s) => s.panel),
        ...(preset.compact?.slots.map((s) => s.panel) ?? []),
        ...preset.spread.map((s) => s.panel),
      ];
      for (const id of ids) {
        const def = getPanel(id);
        if (def) expect({ preset: preset.id, panel: id, phase: def.phase }).toEqual({ preset: preset.id, panel: id, phase: 0 });
      }
    }
  });
});
