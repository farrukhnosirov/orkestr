import { describe, expect, it } from "vitest";

import { motionOffset, scaledTransition } from "./motion";
import { usePerfStore } from "./perfStore";
import { isEndlessDecoration } from "./usePerformanceRuntime";
import { FrameBudgetMonitor, PERF_MODES, canDegrade, effectiveMode, resolvePolicy, rootAttributes, applyPolicyToRoot } from "./policy";

const ENV = { reducedMotion: false, hidden: false };

describe("performance policy", () => {
  it("degrades monotonically from Ultra Visual to Battery Saver", () => {
    const policies = PERF_MODES.map((m) => resolvePolicy(m, ENV));
    for (let i = 1; i < policies.length; i += 1) {
      const [prev, cur] = [policies[i - 1]!, policies[i]!];
      expect(cur.robotFps).toBeLessThanOrEqual(prev.robotFps);
      expect(cur.chartRefreshMs).toBeGreaterThanOrEqual(prev.chartRefreshMs);
      expect(cur.durationScale).toBeLessThanOrEqual(prev.durationScale);
      expect(cur.blurScale).toBeLessThanOrEqual(prev.blurScale);
    }
    expect(policies[0]).toMatchObject({ mode: "UltraVisual", robots3d: true, gpuEffects: true, robotFps: 60 });
    expect(policies[1]).toMatchObject({ mode: "Balanced", robots3d: false, gpuEffects: false, blur: true });
    expect(policies[2]).toMatchObject({ mode: "Performance", blur: false, glow: false, motion: "reduced", backgroundAnimation: false });
    expect(policies[3]).toMatchObject({ mode: "BatterySaver", motion: "off", durationScale: 0, robotFps: 0 });
  });

  it("GPU-heavy effects only in Ultra Visual", () => {
    expect(PERF_MODES.filter((m) => resolvePolicy(m, ENV).gpuEffects)).toEqual(["UltraVisual"]);
  });

  it("prefers-reduced-motion caps motion and stops the background", () => {
    const p = resolvePolicy("UltraVisual", { reducedMotion: true, hidden: false });
    expect(p.motion).toBe("reduced");
    expect(p.durationScale).toBe(0.5);
    expect(p.backgroundAnimation).toBe(false);
    // Already lower levels are untouched.
    expect(resolvePolicy("BatterySaver", { reducedMotion: true, hidden: false }).motion).toBe("off");
  });

  it("a hidden window pauses every visual", () => {
    const p = resolvePolicy("UltraVisual", { reducedMotion: false, hidden: true });
    expect(p).toMatchObject({ paused: true, robotFps: 0, gpuEffects: false, backgroundAnimation: false });
    expect(p.chartRefreshMs).toBeGreaterThanOrEqual(30000);
  });

  it("an unfocused but visible window pauses decoration, keeps data and robots", () => {
    const p = resolvePolicy("UltraVisual", { reducedMotion: false, hidden: false, inactive: true });
    // Idle, not paused: a global CSS pause would freeze entrance animations at opacity 0.
    expect(p).toMatchObject({ idle: true, paused: false, backgroundAnimation: false, gpuEffects: false });
    // Robots are separate windows; charts keep their normal refresh.
    expect(p.robotFps).toBe(60);
    expect(p.chartRefreshMs).toBe(resolvePolicy("UltraVisual", ENV).chartRefreshMs);
  });

  it("switches to Battery Saver on battery only when allowed", () => {
    const base = { chosen: "UltraVisual" as const, autoBattery: true, onBattery: true, autoDegrade: false, degradeSteps: 0 };
    expect(effectiveMode(base)).toEqual({ mode: "BatterySaver", reason: "battery" });
    expect(effectiveMode({ ...base, autoBattery: false })).toEqual({ mode: "UltraVisual", reason: "user" });
    expect(effectiveMode({ ...base, onBattery: null })).toEqual({ mode: "UltraVisual", reason: "user" });
    expect(effectiveMode({ ...base, onBattery: false })).toEqual({ mode: "UltraVisual", reason: "user" });
  });

  it("frame-budget degrade steps down but never below Performance", () => {
    const base = { chosen: "UltraVisual" as const, autoBattery: false, onBattery: null, autoDegrade: true, degradeSteps: 1 };
    expect(effectiveMode(base)).toEqual({ mode: "Balanced", reason: "frameBudget" });
    expect(effectiveMode({ ...base, degradeSteps: 5 })).toEqual({ mode: "Performance", reason: "frameBudget" });
    expect(canDegrade({ ...base, degradeSteps: 2 })).toBe(false);
    expect(effectiveMode({ ...base, chosen: "BatterySaver" })).toEqual({ mode: "BatterySaver", reason: "user" });
    expect(effectiveMode({ ...base, autoDegrade: false })).toEqual({ mode: "UltraVisual", reason: "user" });
  });

  it("FrameBudgetMonitor degrades only on sustained jank", () => {
    const m = new FrameBudgetMonitor({ budgetMs: 20, windowFrames: 10, jankRatio: 0.25, badWindows: 2 });
    const feed = (delta: number, n: number) => {
      let out: "degrade" | null = null;
      for (let i = 0; i < n; i += 1) out = m.push(delta) ?? out;
      return out;
    };
    expect(feed(16, 50)).toBeNull(); // smooth
    expect(feed(40, 10)).toBeNull(); // one bad window
    expect(feed(16, 10)).toBeNull(); // recovered: counter resets
    expect(feed(40, 10)).toBeNull();
    expect(feed(40, 10)).toBe("degrade"); // two bad windows in a row
    expect(feed(1000, 100)).toBeNull(); // pauses (tab switch) are not jank
    expect(m.push(Number.NaN)).toBeNull();
  });

  it("FrameBudgetMonitor degrades from 4 s bursts on a 60 Hz display", () => {
    // Default thresholds, as the runtime uses them: one 4 s burst = one window.
    const m = new FrameBudgetMonitor();
    const burst = (frameMs: number, everyNth = 1) => {
      m.beginWindow();
      let t = 0;
      let i = 0;
      while (t < 4000) {
        const d = i % everyNth === 0 ? frameMs : 1000 / 60;
        m.sample(d);
        t += d;
        i += 1;
      }
      return m.endWindow();
    };
    // 30 fps (UltraVisual struggling under agent load): ~120 frames per burst.
    expect(burst(1000 / 60)).toBeNull(); // smooth 60 Hz
    expect(burst(33)).toBeNull(); // first bad burst
    expect(burst(33)).toBe("degrade"); // second bad burst in a row
    expect(burst(33)).toBeNull(); // the streak starts over after a step down
    expect(burst(1000 / 60)).toBeNull(); // recovered
    expect(burst(33)).toBeNull();
    expect(burst(1000 / 60)).toBeNull(); // a good burst breaks the streak
    expect(burst(33)).toBeNull();
    // A burst cut short (window hidden) is not evaluated and keeps the streak.
    m.beginWindow();
    for (let i = 0; i < 5; i += 1) m.sample(40);
    expect(m.endWindow()).toBeNull();
    expect(burst(25, 2)).toBe("degrade"); // half the frames at 25 ms still miss the budget
  });

  it("writes the root contract", () => {
    const p = resolvePolicy("Performance", ENV);
    const { data, vars } = rootAttributes(p);
    expect(data).toMatchObject({ okPerformance: "Performance", okMotion: "reduced", okBlur: "off", okGlow: "off", okGpu: "off", okPaused: "off" });
    expect(vars["--ok-motion-scale"]).toBe("0.5");
    const root = document.createElement("div");
    applyPolicyToRoot(p, root);
    expect(root.dataset["okBlur"]).toBe("off");
    expect(root.style.getPropertyValue("--ok-blur-scale")).toBe("0");
  });
});

describe("performance store", () => {
  it("derives the effective mode from power and degrade steps", () => {
    const s = usePerfStore.getState();
    s.setAutoBattery(true);
    s.setAutoDegrade(true);
    s.setChosen("UltraVisual");
    s.setPower({ supported: true, hasBattery: true, onAcPower: false, batteryPercent: 40 });
    expect(usePerfStore.getState()).toMatchObject({ mode: "BatterySaver", reason: "battery" });
    s.setPower({ supported: true, hasBattery: true, onAcPower: true, batteryPercent: 40 });
    expect(usePerfStore.getState().mode).toBe("UltraVisual");
    expect(usePerfStore.getState().degrade()).toBe(true);
    expect(usePerfStore.getState()).toMatchObject({ mode: "Balanced", reason: "frameBudget" });
    // A new explicit choice forgets automatic steps.
    s.setChosen("Balanced");
    s.setChosen("UltraVisual");
    expect(usePerfStore.getState()).toMatchObject({ mode: "UltraVisual", degradeSteps: 0 });
    s.setHidden(true);
    expect(usePerfStore.getState().policy.paused).toBe(true);
    s.setHidden(false);
    s.setAutoDegrade(false);
    expect(usePerfStore.getState().degrade()).toBe(false);
  });

  it("only endless non-spinner animations count as idle decoration", () => {
    const anim = (iterations: number, cls: string) => {
      const el = document.createElement("div");
      el.className = cls;
      const effect = { target: el, getComputedTiming: () => ({ iterations }) };
      return { effect } as unknown as Animation;
    };
    expect(isEndlessDecoration(anim(Infinity, "ok-empty__icon"))).toBe(true);
    // One-shot entrance animation: must never be paused (it would stay at opacity 0).
    expect(isEndlessDecoration(anim(1, "overlay"))).toBe(false);
    expect(isEndlessDecoration(anim(Infinity, "ok-spinner"))).toBe(false);
  });

  it("an unchanged power reading keeps the same policy object", () => {
    const s = usePerfStore.getState();
    const reading = { supported: true, hasBattery: false, onAcPower: true, batteryPercent: null };
    s.setPower(reading);
    const before = usePerfStore.getState().policy;
    // Polled every minute and on every window focus: must not churn subscribers.
    s.setPower({ ...reading });
    expect(usePerfStore.getState().policy).toBe(before);
  });

  it("an unfocused window pauses decoration and resumes on focus", () => {
    const s = usePerfStore.getState();
    s.setChosen("UltraVisual");
    s.setInactive(true);
    expect(usePerfStore.getState().policy).toMatchObject({ idle: true, paused: false, backgroundAnimation: false, gpuEffects: false });
    s.setInactive(false);
    expect(usePerfStore.getState().policy).toMatchObject({ idle: false, paused: false, backgroundAnimation: true });
  });
});

describe("motion system", () => {
  it("scales durations with the policy and goes instant at 0", () => {
    expect(scaledTransition(0)).toEqual({ duration: 0 });
    expect(scaledTransition(0.5, "base")).toMatchObject({ duration: 0.1 });
    expect(scaledTransition(1, "base", true)).toMatchObject({ type: "spring" });
    expect(scaledTransition(0.5, "base", true)).not.toHaveProperty("type");
    expect([motionOffset("full"), motionOffset("reduced"), motionOffset("off")]).toEqual([10, 4, 0]);
  });

});
