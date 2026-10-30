import { beforeEach, describe, expect, it } from "vitest";

import { DEFAULT_BOOT_ENV, ENV_KEY, isFirstLaunch, PREFS_KEY, readBootEnv, readBootPrefs, shortVersion, writeBootEnv, writeBootPrefs } from "./prefs";
import { BootSequencer, chooseVariant, minDuration, SKIP_EXIT_S, TIMELINES, typed, type VariantInput } from "./sequence";
import { bootSoundAllowed } from "./sound";
import { stepsProgress, stepsSettled } from "./store";

const base: VariantInput = { pref: "always", firstLaunch: false, reducedMotion: false, performanceMode: "Balanced", hidden: false };

describe("variant selection", () => {
  it("full after install / update, short otherwise", () => {
    expect(chooseVariant({ ...base, firstLaunch: true })).toBe("full");
    expect(chooseVariant(base)).toBe("short");
  });
  it("«only after install and updates» skips regular starts", () => {
    expect(chooseVariant({ ...base, pref: "firstLaunch" })).toBe("none");
    expect(chooseVariant({ ...base, pref: "firstLaunch", firstLaunch: true })).toBe("full");
  });
  it("off, or a hidden (autostart) window: nothing", () => {
    expect(chooseVariant({ ...base, pref: "off", firstLaunch: true })).toBe("none");
    expect(chooseVariant({ ...base, hidden: true })).toBe("none");
  });
  it("reduced motion and the light performance modes play the static version", () => {
    expect(chooseVariant({ ...base, firstLaunch: true, reducedMotion: true })).toBe("static");
    expect(chooseVariant({ ...base, performanceMode: "Performance" })).toBe("static");
    expect(chooseVariant({ ...base, performanceMode: "BatterySaver" })).toBe("static");
    expect(chooseVariant({ ...base, performanceMode: "UltraVisual" })).toBe("short");
  });
  it("durations: full 3.5–5 s, short 1.5–2 s", () => {
    expect(minDuration("full")).toBeGreaterThanOrEqual(3.5);
    expect(minDuration("full")).toBeLessThanOrEqual(5);
    expect(minDuration("short")).toBeGreaterThanOrEqual(1.5);
    expect(minDuration("short")).toBeLessThanOrEqual(2);
    expect(minDuration("static")).toBeLessThan(minDuration("short"));
  });
});

describe("sequencer", () => {
  it("core ready early: ends exactly at the animation minimum", () => {
    const s = new BootSequencer("short", 0);
    s.markReady(100);
    expect(s.state(500).phase).toBe("intro");
    expect(s.state(1000).phase).toBe("online");
    expect(s.state(1500).phase).toBe("exit");
    expect(s.state(minDuration("short") * 1000 - 1).phase).toBe("exit");
    expect(s.state(minDuration("short") * 1000 + 1).phase).toBe("done");
  });

  it("core slower than the animation: holds at the gate, then plays the rest", () => {
    const s = new BootSequencer("short", 0);
    const gate = TIMELINES.short.gate;
    const hold = s.state(3000);
    expect(hold.phase).toBe("holding");
    expect(hold.t).toBe(gate);
    expect(hold.hold).toBeCloseTo(3 - gate);
    s.markReady(3000);
    const after = s.state(3100);
    expect(after.phase).toBe("online");
    expect(after.t).toBeCloseTo(gate + 0.1);
    // Rest of the sequence after readiness: exitAt - gate + exitDur.
    const rest = (TIMELINES.short.exitAt - gate + TIMELINES.short.exitDur) * 1000;
    expect(s.state(3000 + rest - 5).phase).toBe("exit");
    expect(s.state(3000 + rest + 5).phase).toBe("done");
  });

  it("the full version waits briefly for the other startup steps (bounded grace)", () => {
    const s = new BootSequencer("full", 0);
    s.markReady(500);
    const gateMs = TIMELINES.full.gate * 1000;
    // Steps not settled yet; grace (600 ms after readiness) already over at the gate.
    expect(s.state(gateMs + 10).phase).toBe("online");

    const late = new BootSequencer("full", 0);
    late.markReady(gateMs + 100);
    expect(late.state(gateMs + 200).phase).toBe("holding");
    late.markSettled(gateMs + 300);
    expect(late.state(gateMs + 310).phase).toBe("online");

    const never = new BootSequencer("full", 0);
    never.markReady(gateMs + 100);
    expect(never.state(gateMs + 600).phase).toBe("holding");
    expect(never.state(gateMs + 100 + TIMELINES.full.settleGraceMs + 1).phase).toBe("online");
  });

  it("skip: quick exit, flying to the title bar only when the app is ready", () => {
    const s = new BootSequencer("full", 0);
    s.skip(400);
    const st = s.state(450);
    expect(st.phase).toBe("exit");
    expect(st.fly).toBe(false);
    expect(s.state(400 + SKIP_EXIT_S * 1000 + 1).phase).toBe("done");

    const r = new BootSequencer("full", 0);
    r.markReady(100);
    r.skip(1000);
    expect(r.state(1100).fly).toBe(true);
    expect(r.state(1000 + SKIP_EXIT_S * 1000 + 1).phase).toBe("done");
  });

  it("skip during the regular exit shortens it", () => {
    const s = new BootSequencer("full", 0);
    s.markReady(0);
    s.markSettled(0);
    const exitStart = TIMELINES.full.exitAt * 1000;
    expect(s.state(exitStart + 10).phase).toBe("exit");
    s.skip(exitStart + 20);
    expect(s.state(exitStart + 20 + SKIP_EXIT_S * 1000 + 1).phase).toBe("done");
  });

  it("a failed start fades out without flying", () => {
    const s = new BootSequencer("short", 0);
    s.fail(300);
    expect(s.state(350)).toMatchObject({ phase: "exit", fly: false });
    expect(s.state(300 + SKIP_EXIT_S * 1000 + 1).phase).toBe("done");
  });

  it("upgrades short → full only at the very start", () => {
    const s = new BootSequencer("short", 0);
    expect(s.upgrade("full", 200)).toBe(true);
    expect(s.timeline).toBe(TIMELINES.full);
    const late = new BootSequencer("short", 0);
    expect(late.upgrade("full", 900)).toBe(false);
  });
});

describe("typing", () => {
  it("types out at the given speed", () => {
    expect(typed("Hello", 0.5, 1)).toBe("");
    expect(typed("Hello", 1.1, 1, 20)).toBe("He");
    expect(typed("Hello", 9, 1)).toBe("Hello");
  });
});

describe("preferences and first launch", () => {
  beforeEach(() => window.localStorage.clear());

  it("defaults: animation always, sound after install / updates, no voice", () => {
    expect(readBootPrefs()).toEqual({ animation: "always", sound: "firstLaunch", voice: false });
    writeBootPrefs({ animation: "off", sound: "always", voice: true });
    expect(readBootPrefs()).toEqual({ animation: "off", sound: "always", voice: true });
  });

  it("ignores broken storage values", () => {
    window.localStorage.setItem(PREFS_KEY, JSON.stringify({ animation: "loud", voice: "yes" }));
    expect(readBootPrefs()).toEqual({ animation: "always", sound: "firstLaunch", voice: false });
    window.localStorage.setItem(ENV_KEY, "{not json");
    expect(readBootEnv()).toEqual(DEFAULT_BOOT_ENV);
  });

  it("mirrors the environment", () => {
    writeBootEnv({ volumePct: 30, performanceMode: "BatterySaver", address: "Boss" });
    writeBootEnv({ doNotDisturb: true });
    expect(readBootEnv()).toMatchObject({ volumePct: 30, performanceMode: "BatterySaver", address: "Boss", doNotDisturb: true });
  });

  it("first launch: nothing seen, another version, or the core's startup check says so", () => {
    expect(isFirstLaunch("1.3.0", null)).toBe(true);
    expect(isFirstLaunch("1.3.0", "1.2.0")).toBe(true);
    expect(isFirstLaunch("1.3.0", "1.3.0")).toBe(false);
    expect(isFirstLaunch("1.3.0", "1.3.0", "upgraded")).toBe(true);
    expect(isFirstLaunch("1.3.0", "1.3.0", "same")).toBe(false);
    expect(shortVersion("1.3.0")).toBe("1.3");
  });
});

describe("boot sound policy", () => {
  const env = { ...DEFAULT_BOOT_ENV };
  const noon = new Date(2026, 9, 30, 12, 0);
  it("default: only on the first launch", () => {
    expect(bootSoundAllowed("firstLaunch", true, env, noon)).toBe(true);
    expect(bootSoundAllowed("firstLaunch", false, env, noon)).toBe(false);
    expect(bootSoundAllowed("always", false, env, noon)).toBe(true);
    expect(bootSoundAllowed("off", true, env, noon)).toBe(false);
  });
  it("respects the sound settings, Do Not Disturb, quiet hours and Battery Saver", () => {
    expect(bootSoundAllowed("always", true, { ...env, soundEnabled: false }, noon)).toBe(false);
    expect(bootSoundAllowed("always", true, { ...env, volumePct: 0 }, noon)).toBe(false);
    expect(bootSoundAllowed("always", true, { ...env, doNotDisturb: true }, noon)).toBe(false);
    expect(bootSoundAllowed("always", true, { ...env, doNotDisturb: true, muteInDnd: false }, noon)).toBe(true);
    const quiet = { enabled: true, start: "22:00", end: "07:00" };
    expect(bootSoundAllowed("always", true, { ...env, quietHours: quiet }, new Date(2026, 9, 30, 23, 30))).toBe(false);
    expect(bootSoundAllowed("always", true, { ...env, quietHours: quiet }, noon)).toBe(true);
    expect(bootSoundAllowed("always", true, { ...env, performanceMode: "BatterySaver" }, noon)).toBe(false);
  });
});

describe("startup steps", () => {
  it("progress and settling", () => {
    const steps = { core: "ok", providers: "pending", voice: "ok", nodes: "error", plugins: "pending" } as const;
    expect(stepsProgress(steps)).toBeCloseTo(0.6);
    expect(stepsSettled(steps)).toBe(false);
    expect(stepsSettled({ ...steps, providers: "ok", plugins: "skipped" })).toBe(true);
  });
});
