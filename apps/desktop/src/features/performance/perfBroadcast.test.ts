/** The orb window follows the main window's effective mode, not the raw user choice. */
import type { PerformanceMode } from "@orkestr/protocol";
import { afterEach, describe, expect, it, vi } from "vitest";

import { resolvePolicy } from "./policy";
import { startPerfBroadcast, subscribeEffectiveMode } from "./perfBroadcast";
import { usePerfStore } from "./perfStore";
import { orbFps, orbMotion } from "../orb/model";

const offs: (() => void)[] = [];
afterEach(() => {
  while (offs.length) offs.pop()?.();
});

describe("effective mode broadcast", () => {
  it("delivers auto Battery Saver on battery to another window", async () => {
    usePerfStore.getState().setChosen("UltraVisual");
    usePerfStore.getState().setAutoBattery(true);
    usePerfStore.getState().setPower({ supported: true, hasBattery: true, onAcPower: true } as never);
    offs.push(startPerfBroadcast());
    const seen: PerformanceMode[] = [];
    offs.push(subscribeEffectiveMode((m) => seen.push(m)));
    await vi.waitFor(() => expect(seen.at(-1)).toBe("UltraVisual"));
    usePerfStore.getState().setPower({ supported: true, hasBattery: true, onAcPower: false } as never);
    await vi.waitFor(() => expect(seen.at(-1)).toBe("BatterySaver"));
    // What the orb window then renders with: a still sphere.
    const mode = seen.at(-1) ?? "UltraVisual";
    const env = { pref: "Full", reducedMotion: false, hidden: false, appActive: true, hovered: false, hubOpen: false, voice: false } as const;
    expect(orbMotion({ ...env, mode })).toBe("static");
    expect(orbFps(orbMotion({ ...env, mode }), mode)).toBe(0);
    expect(orbFps(orbMotion({ ...env, mode: "UltraVisual" }), "UltraVisual")).toBeGreaterThan(0);
    expect(resolvePolicy(mode, { reducedMotion: false, hidden: false }).robots3d).toBe(false);
  });
});
