/** Jarvis voice visual: state machine, dynamics, spectrum, typing, frames. */
import { describe, expect, it } from "vitest";

import { channelSignal, isSignalFrame, toFrame } from "./frame";
import {
  BANDS,
  bandsFromFrequency,
  clampJarvisSize,
  follow,
  jarvisStateOf,
  reactorFps,
  ReactorDynamics,
  snapToWord,
  splitWords,
  statusKey,
  syntheticBands,
  typedChars,
  WAKE_INTRO_MS,
} from "./model";

describe("jarvis state machine", () => {
  it("maps pipeline phases to visual states", () => {
    expect(jarvisStateOf({ phase: "idle", speaking: false })).toBe("idle");
    expect(jarvisStateOf({ phase: "listening", speaking: false, source: "PushToTalk", sinceListenMs: 10 })).toBe("listening");
    expect(jarvisStateOf({ phase: "listening", speaking: false, source: "WakeWord", sinceListenMs: 10 })).toBe("wake");
    expect(jarvisStateOf({ phase: "listening", speaking: false, source: "WakeWord", sinceListenMs: WAKE_INTRO_MS + 1 })).toBe("listening");
    expect(jarvisStateOf({ phase: "transcribing", speaking: false })).toBe("thinking");
    expect(jarvisStateOf({ phase: "thinking", speaking: false })).toBe("thinking");
    expect(jarvisStateOf({ phase: "result", speaking: true })).toBe("speaking");
    expect(jarvisStateOf({ phase: "result", speaking: false })).toBe("done");
    expect(jarvisStateOf({ phase: "confirm", speaking: false })).toBe("listening");
    expect(jarvisStateOf({ phase: "error", speaking: true })).toBe("error");
    // Speaking a reply while idle (manager reply arrives later) still animates.
    expect(jarvisStateOf({ phase: "idle", speaking: true })).toBe("speaking");
  });

  it("status line keys", () => {
    expect(statusKey("listening")).toBe("status.listening");
    expect(statusKey("wake")).toBe("status.listening");
    expect(statusKey("thinking", "transcribing")).toBe("status.transcribing");
    expect(statusKey("listening", "confirm")).toBe("status.confirm");
    expect(statusKey("speaking")).toBe("status.speaking");
    expect(statusKey("error")).toBe("status.error");
  });

  it("frame budget: 60 fps, lower in Performance, still when reduced / battery / hidden", () => {
    expect(reactorFps("Balanced", false, false)).toBe(60);
    expect(reactorFps("UltraVisual", false, false)).toBe(60);
    expect(reactorFps("Performance", false, false)).toBe(24);
    expect(reactorFps("BatterySaver", false, false)).toBe(0);
    expect(reactorFps("Balanced", true, false)).toBe(0);
    expect(reactorFps("Balanced", false, true)).toBe(0);
  });

  it("size setting clamps to 120..180", () => {
    expect(clampJarvisSize(100)).toBe(120);
    expect(clampJarvisSize(500)).toBe(180);
    expect(clampJarvisSize("x")).toBe(150);
    expect(clampJarvisSize(150)).toBe(150);
  });
});

describe("reactor dynamics", () => {
  const run = (d: ReactorDynamics, from: number, ms: number, state: Parameters<ReactorDynamics["step"]>[2], level: number) => {
    let f = d.step(from, 16, state, level, null);
    for (let t = from + 16; t <= from + ms; t += 16) f = d.step(t, 16, state, level, null);
    return f;
  };

  it("wake plays the bloom intro, then settles at full scale", () => {
    const d = new ReactorDynamics();
    const first = d.step(1000, 16, "wake", 0.2, null);
    expect(first.intro).toBeLessThan(0.1);
    expect(first.scale).toBeLessThan(0.6);
    const settled = run(d, 1016, 2000, "listening", 0.2);
    expect(settled.intro).toBe(1);
    expect(settled.scale).toBeGreaterThan(0.97);
    expect(settled.energy).toBeGreaterThan(0.95);
  });

  it("level attacks fast and decays slowly", () => {
    const d = new ReactorDynamics();
    run(d, 0, 200, "listening", 0);
    const up = run(d, 300, 100, "listening", 1);
    expect(up.level).toBeGreaterThan(0.8);
    const down = run(d, 500, 100, "listening", 0);
    expect(down.level).toBeGreaterThan(0.3);
    expect(follow(0, 1, 16, 0, 0)).toBe(1);
  });

  it("thinking shows the sweep, speaking the ripples, error the alarm, done dims", () => {
    const d = new ReactorDynamics();
    expect(run(d, 0, 1000, "thinking", 0).sweepAlpha).toBeGreaterThan(0.9);
    expect(run(d, 2000, 1000, "speaking", 0.6).rippleAlpha).toBeGreaterThan(0.9);
    expect(run(d, 4000, 1000, "error", 0).alarm).toBeGreaterThan(0.9);
    const done = run(d, 6000, 2000, "done", 0);
    expect(done.energy).toBeLessThan(0.7);
    expect(done.scale).toBeLessThan(0.95);
  });

  it("live bands drive the spectrum while listening", () => {
    const d = new ReactorDynamics();
    const raw = new Float32Array(BANDS).fill(0);
    raw[5] = 1;
    let f = d.step(0, 16, "listening", 0.5, raw);
    for (let t = 16; t < 400; t += 16) f = d.step(t, 16, "listening", 0.5, raw);
    expect(f.bands[5]).toBeGreaterThan(0.9);
    expect(f.bands[20]).toBeLessThan(0.05);
  });

  it("still frame for reduced motion", () => {
    const f = new ReactorDynamics().still("thinking", 0);
    expect(f.intro).toBe(1);
    expect(f.sweepAlpha).toBe(1);
  });
});

describe("spectrum", () => {
  it("bands from frequency data stay in 0..1 and follow energy", () => {
    const data = new Uint8Array(512);
    const out = new Float32Array(BANDS);
    bandsFromFrequency(data, out);
    expect(Math.max(...out)).toBe(0);
    // 1 kHz tone at 48 kHz / 512 bins → bin ≈ 21.
    data[21] = 255;
    bandsFromFrequency(data, out);
    expect(Math.max(...out)).toBeGreaterThan(0.5);
    expect(out.every((v) => v >= 0 && v <= 1)).toBe(true);
  });

  it("synthetic bands scale with the level", () => {
    const a = new Float32Array(BANDS);
    const b = new Float32Array(BANDS);
    syntheticBands(0, 1, a);
    syntheticBands(0.8, 1, b);
    expect(Math.max(...a)).toBe(0);
    expect(Math.max(...b)).toBeGreaterThan(0.3);
  });

  it("channel frames round-trip as plain numbers", () => {
    const bands = new Float32Array(BANDS).map((_, i) => i / BANDS);
    const frame = toFrame({ level: 0.4567, progress: 0.25 }, bands);
    expect(isSignalFrame(frame)).toBe(true);
    expect(isSignalFrame({ level: "x" })).toBe(false);
    expect(JSON.parse(JSON.stringify(frame))).toEqual(frame);
    const sig = channelSignal();
    const out = new Float32Array(BANDS);
    expect(sig.sample(out, performance.now())).toBeNull();
    sig.push(frame);
    const s = sig.sample(out, performance.now());
    expect(s?.level).toBeCloseTo(0.46, 2);
    expect(s?.progress).toBe(0.25);
    expect(out[16]).toBeCloseTo(0.5, 2);
    expect(sig.sample(out, performance.now() + 1000)).toBeNull();
  });
});

describe("text", () => {
  it("splits words keeping spaces", () => {
    expect(splitWords("открой  YouTube ")).toEqual(["открой  ", "YouTube "]);
    expect(splitWords("")).toEqual([]);
  });

  it("types the reply: by time, by speech progress, all at once without motion", () => {
    const base = { length: 100, speaking: false, progress: null, animate: true };
    expect(typedChars({ ...base, elapsedMs: 0 })).toBe(0);
    expect(typedChars({ ...base, elapsedMs: 1000 })).toBe(55);
    expect(typedChars({ ...base, elapsedMs: 10_000 })).toBe(100);
    expect(typedChars({ ...base, elapsedMs: 0, animate: false })).toBe(100);
    // Follows the voice, a word ahead.
    expect(typedChars({ ...base, speaking: true, progress: 0.5, elapsedMs: 100 })).toBe(58);
    expect(snapToWord("Открываю YouTube, сэр.", 3)).toBe(8);
    expect(snapToWord("abc", 10)).toBe(3);
  });
});
