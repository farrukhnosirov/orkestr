/** Robot windows follow the main window's effective mode, not the raw user choice. */
import type { PerformanceMode } from "@orkestr/protocol";
import { afterEach, describe, expect, it, vi } from "vitest";

import { resolvePolicy } from "./policy";
import { startPerfBroadcast, subscribeEffectiveMode } from "./perfBroadcast";
import { usePerfStore } from "./perfStore";
import { robotEffects, robotFps } from "../robots/model/perf";

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
    // What a robot window then renders with.
    const mode = seen.at(-1) ?? "UltraVisual";
    const busy = { state: "Coding", reducedMotion: false, hidden: false, intensity: 1, asleep: false, hovered: false } as const;
    expect(robotFps({ ...busy, mode })).toBeLessThan(robotFps({ ...busy, mode: "UltraVisual" }));
    expect(robotEffects(mode, false).threeD).toBe(false);
    expect(resolvePolicy(mode, { reducedMotion: false, hidden: false }).robots3d).toBe(false);
  });
});
