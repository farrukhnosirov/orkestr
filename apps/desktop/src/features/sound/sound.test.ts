import type { SoundConfig } from "@orkestr/protocol";
import { describe, expect, it } from "vitest";

import { masterGain, shouldPlaySound, soundQuality } from "./policy";
import { presetForTheme, RESOLVED_PRESETS, recipeDuration, recipeFor, resolvePreset, SOUND_CUES } from "./recipes";

const CONFIG: SoundConfig = { enabled: true, volumePct: 60, preset: "Auto", muteInDnd: true };
const ctx = { config: CONFIG, doNotDisturb: false, performanceMode: "Balanced" as const };

describe("sound policy", () => {
  it("plays by default and obeys the master switch / zero volume", () => {
    expect(shouldPlaySound("completion", ctx)).toBe(true);
    expect(shouldPlaySound("completion", { ...ctx, config: { ...CONFIG, enabled: false } })).toBe(false);
    expect(shouldPlaySound("alert", { ...ctx, config: { ...CONFIG, volumePct: 0 } })).toBe(false);
  });

  it("mutes in DND except critical alerts; respects the mute-in-DND switch", () => {
    expect(shouldPlaySound("completion", { ...ctx, doNotDisturb: true })).toBe(false);
    expect(shouldPlaySound("alert", { ...ctx, doNotDisturb: true })).toBe(true);
    expect(shouldPlaySound("completion", { ...ctx, doNotDisturb: true, config: { ...CONFIG, muteInDnd: false } })).toBe(true);
  });

  it("battery saver keeps only feedback and critical cues", () => {
    const bs = { ...ctx, performanceMode: "BatterySaver" as const };
    expect(shouldPlaySound("completion", bs)).toBe(false);
    expect(shouldPlaySound("deploy", bs)).toBe(false);
    expect(shouldPlaySound("wake", bs)).toBe(true);
    expect(shouldPlaySound("alert", bs)).toBe(true);
  });

  it("settings test ignores DND but not the master switch", () => {
    expect(shouldPlaySound("completion", { ...ctx, doNotDisturb: true, test: true })).toBe(true);
    expect(shouldPlaySound("completion", { ...ctx, config: { ...CONFIG, enabled: false }, test: true })).toBe(false);
  });

  it("quality follows the performance mode", () => {
    expect(soundQuality("UltraVisual")).toBe("full");
    expect(soundQuality("Balanced")).toBe("full");
    expect(soundQuality("Performance")).toBe("lite");
    expect(soundQuality("BatterySaver")).toBe("lite");
  });

  it("master gain is a clamped perceptual curve", () => {
    expect(masterGain(0)).toBe(0);
    expect(masterGain(100)).toBe(1);
    expect(masterGain(50)).toBeCloseTo(0.25);
    expect(masterGain(500)).toBe(1);
    expect(masterGain(-5)).toBe(0);
  });
});

describe("sound recipes", () => {
  it("every cue × preset × quality is short, audible and bounded", () => {
    for (const cue of SOUND_CUES) {
      for (const preset of RESOLVED_PRESETS) {
        for (const quality of ["full", "lite"] as const) {
          const r = recipeFor(cue, preset, quality);
          expect(r.tones.length).toBeGreaterThan(0);
          expect(recipeDuration(r)).toBeLessThan(1.5);
          for (const tone of r.tones) {
            expect(tone.freq).toBeGreaterThan(20);
            expect(tone.freq).toBeLessThan(20_000);
            expect(tone.gain).toBeGreaterThan(0);
            expect(tone.gain).toBeLessThanOrEqual(1);
          }
          if (quality === "lite") {
            expect(r.reverb).toBe(0);
            expect(r.noise).toBeUndefined();
          }
        }
      }
    }
  });

  it("lite quality uses fewer voices than full", () => {
    const full = recipeFor("completion", "Nebula", "full");
    const lite = recipeFor("completion", "Nebula", "lite");
    expect(lite.tones.length).toBeLessThan(full.tones.length);
  });

  it("cues are distinguishable (different melodies)", () => {
    const sig = (cue: (typeof SOUND_CUES)[number]) =>
      recipeFor(cue, "Crystal", "lite")
        .tones.map((t) => Math.round(t.freq))
        .join(",");
    const sigs = new Set(SOUND_CUES.map(sig));
    expect(sigs.size).toBe(SOUND_CUES.length);
  });

  it("Auto follows the theme; unknown themes get a stable preset", () => {
    expect(resolvePreset("Auto", "nebula")).toBe("Nebula");
    expect(resolvePreset("Crystal", "nebula")).toBe("Crystal");
    expect(presetForTheme("my-theme")).toBe(presetForTheme("my-theme"));
    expect(RESOLVED_PRESETS).toContain(presetForTheme("my-theme"));
  });
});
