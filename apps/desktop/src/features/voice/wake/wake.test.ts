import { describe, expect, it } from "vitest";

import { dtwDistance, meanPairwiseDistance, normalizeFeatures, thresholdFor } from "./dtw";
import { fft, FRAME_LEN, frameEnergyDb, MfccExtractor, NUM_CEPS, SAMPLE_RATE } from "./mfcc";
import { FrameSegmenter, segmentFrames } from "./segmenter";
import { checkEnrollSample, consistentWith, WakeDetector, WakeSpotter } from "./spotter";
import { EnergyVad } from "./vad";

/** Seeded PRNG (mulberry32). */
function rng(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/**
 * Synthetic "word": smooth cepstral trajectories defined by `shape`, time-
 * stretched to `frames`, with per-frame noise and a loudness offset on c0
 * and a constant channel offset on every coefficient (removed by CMN).
 */
function word(shape: number, frames: number, seed: number, noise = 0.35, channel = 0): number[][] {
  const r = rng(seed);
  const out: number[][] = [];
  for (let i = 0; i < frames; i++) {
    const x = i / Math.max(1, frames - 1);
    const f: number[] = [20 + r() * 3];
    for (let d = 1; d < NUM_CEPS; d++) {
      const v = 6 * Math.sin(2 * Math.PI * (x * (1 + ((shape * d) % 3)) + shape * 0.37 * d)) + 3 * Math.cos(Math.PI * x * d * (0.5 + shape * 0.2));
      f.push(v + (r() - 0.5) * 2 * noise + channel);
    }
    out.push(f);
  }
  return out;
}

describe("MFCC", () => {
  it("FFT of a cosine peaks at its bin", () => {
    const n = 64;
    const re = new Float32Array(n);
    const im = new Float32Array(n);
    for (let i = 0; i < n; i++) re[i] = Math.cos((2 * Math.PI * 5 * i) / n);
    fft(re, im);
    const mags = Array.from(re, (r, i) => Math.hypot(r, im[i] ?? 0));
    expect(mags.indexOf(Math.max(...mags.slice(0, n / 2)))).toBe(5);
  });

  it("is deterministic, finite and distinguishes tones", () => {
    const ex = new MfccExtractor();
    const tone = (hz: number) => Float32Array.from({ length: FRAME_LEN }, (_, i) => 0.3 * Math.sin((2 * Math.PI * hz * i) / SAMPLE_RATE));
    const a = Array.from(ex.compute(tone(300)));
    const b = Array.from(ex.compute(tone(300)));
    const c = Array.from(ex.compute(tone(2500)));
    expect(a).toHaveLength(NUM_CEPS);
    expect(a.every(Number.isFinite)).toBe(true);
    expect(a).toEqual(b);
    const diff = a.slice(1).reduce((s, v, i) => s + Math.abs(v - (c[i + 1] ?? 0)), 0);
    expect(diff).toBeGreaterThan(5);
  });

  it("measures energy in dBFS", () => {
    expect(frameEnergyDb(new Float32Array(400).fill(1))).toBeCloseTo(0, 3);
    expect(frameEnergyDb(new Float32Array(400))).toBeLessThan(-100);
  });
});

describe("energy VAD", () => {
  it("starts after the onset frames and ends after the hangover", () => {
    const vad = new EnergyVad();
    const events: Array<[number, string]> = [];
    const feed = (db: number, n: number) => {
      for (let i = 0; i < n; i++) {
        const e = vad.push(db);
        if (e) events.push([events.length, e]);
      }
    };
    feed(-65, 50); // room noise establishes the floor
    feed(-25, 40); // speech
    feed(-65, 40); // silence
    expect(events.map((e) => e[1])).toEqual(["start", "end"]);
    expect(vad.noiseFloorDb).toBeLessThan(-55);
  });

  it("ignores short clicks", () => {
    const vad = new EnergyVad();
    const seen: string[] = [];
    for (let i = 0; i < 50; i++) vad.push(-65);
    for (let i = 0; i < 2; i++) {
      const e = vad.push(-20);
      if (e) seen.push(e);
    }
    for (let i = 0; i < 30; i++) {
      const e = vad.push(-65);
      if (e) seen.push(e);
    }
    expect(seen).toEqual([]);
  });
});

describe("DTW", () => {
  it("is zero for identical sequences and tolerant to time warping", () => {
    const a = normalizeFeatures(word(1, 60, 1, 0));
    expect(dtwDistance(a, a)).toBe(0);
    const stretched = normalizeFeatures(word(1, 78, 2));
    const other = normalizeFeatures(word(2.3, 64, 3));
    expect(dtwDistance(a, stretched)).toBeLessThan(dtwDistance(a, other) * 0.5);
  });

  it("cepstral mean normalization removes channel offsets", () => {
    const a = normalizeFeatures(word(1, 60, 1, 0));
    const b = normalizeFeatures(word(1, 60, 1, 0, 4));
    expect(dtwDistance(a, b)).toBeCloseTo(0, 6);
  });

  it("open-end matching finds the word at the start of a longer utterance", () => {
    const t = normalizeFeatures(word(1, 60, 1));
    const utterance = [...word(1, 64, 9), ...word(2.7, 90, 10)];
    const open = dtwDistance(t, normalizeFeatures(utterance, 80), true);
    const closed = dtwDistance(t, normalizeFeatures(utterance));
    expect(open).toBeLessThan(closed);
  });

  it("threshold grows with sensitivity", () => {
    expect(thresholdFor(2, 0)).toBeCloseTo(2.2);
    expect(thresholdFor(2, 100)).toBeCloseTo(3.8);
    expect(thresholdFor(2, 50)).toBeGreaterThan(thresholdFor(2, 10));
  });
});

describe("WakeSpotter", () => {
  const enroll = [word(1, 58, 11), word(1, 66, 12), word(1, 61, 13)];

  it("accepts the enrolled word (warped, noisy, different gain) and rejects others", () => {
    const spotter = new WakeSpotter(enroll, 50);
    expect(spotter.ready).toBe(true);
    expect(spotter.detect(word(1, 72, 21, 0.4, 3), 10_000)).not.toBeNull();
    const fresh = new WakeSpotter(enroll, 50);
    for (const [shape, seed] of [
      [2.3, 31],
      [0.4, 32],
      [3.1, 33],
      [1.7, 34],
    ] as const) {
      expect(fresh.detect(word(shape, 60, seed), 10_000)).toBeNull();
    }
  });

  it("detects the wake word followed by a command without a pause", () => {
    const spotter = new WakeSpotter(enroll, 50);
    const utterance = [...word(1, 63, 41), ...word(2.9, 100, 42)];
    expect(spotter.detect(utterance, 10_000, true)).not.toBeNull();
  });

  it("has a cooldown after a hit", () => {
    const spotter = new WakeSpotter(enroll, 50);
    expect(spotter.detect(word(1, 60, 51), 10_000)).not.toBeNull();
    expect(spotter.detect(word(1, 60, 52), 11_000)).toBeNull();
    expect(spotter.detect(word(1, 60, 53), 13_000)).not.toBeNull();
  });

  it("is not ready with too few samples", () => {
    expect(new WakeSpotter(enroll.slice(0, 2), 50).ready).toBe(false);
  });

  it("validates enrollment samples", () => {
    expect(checkEnrollSample(10, -20)).toBe("tooShort");
    expect(checkEnrollSample(400, -20)).toBe("tooLong");
    expect(checkEnrollSample(60, -60)).toBe("tooQuiet");
    expect(checkEnrollSample(60, -20)).toBeNull();
    const accepted = enroll.slice(0, 2).map((s) => normalizeFeatures(s));
    expect(consistentWith(normalizeFeatures(word(1, 63, 61)), accepted)).toBe(true);
    expect(consistentWith(normalizeFeatures(word(2.6, 63, 62)), accepted)).toBe(false);
    expect(Number.isNaN(meanPairwiseDistance([]))).toBe(true);
  });
});

describe("FrameSegmenter", () => {
  it("emits a speech segment with pre-roll and without the silent tail", () => {
    const seg = new FrameSegmenter();
    const mfcc = new Float32Array(NUM_CEPS);
    const out = [];
    for (let i = 0; i < 40; i++) seg.push(-65, mfcc);
    for (let i = 0; i < 50; i++) {
      const s = seg.push(-25, mfcc.map(() => i));
      if (s) out.push(s);
    }
    for (let i = 0; i < 40; i++) {
      const s = seg.push(-65, mfcc);
      if (s) out.push(s);
    }
    expect(out).toHaveLength(1);
    const s = out[0];
    expect(s?.early).toBe(false);
    expect(s?.frames).toBeGreaterThanOrEqual(50);
    expect(s?.frames).toBeLessThan(70);
    expect(segmentFrames(s ?? { data: new Float32Array(), frames: 0 })).toHaveLength(s?.frames ?? -1);
  });

  it("emits early for long utterances, once", () => {
    const seg = new FrameSegmenter();
    const mfcc = new Float32Array(NUM_CEPS);
    const out = [];
    for (let i = 0; i < 40; i++) seg.push(-65, mfcc);
    for (let i = 0; i < 400; i++) {
      const s = seg.push(-25, mfcc);
      if (s) out.push(s);
    }
    for (let i = 0; i < 40; i++) {
      const s = seg.push(-65, mfcc);
      if (s) out.push(s);
    }
    expect(out.map((s) => s.early)).toEqual([true]);
    expect(out[0]?.frames).toBe(160);
  });
});

describe("WakeDetector (several wake phrases, ADR-0056)", () => {
  const jarvis = { phrase: "Джарвис", samples: [word(1, 58, 11), word(1, 66, 12), word(1, 61, 13)] };
  const hey = { phrase: "Эй, Джарвис", samples: [word(2.3, 70, 61), word(2.3, 75, 62), word(2.3, 72, 63)] };

  it("detects any recorded phrase and names it", () => {
    const d = new WakeDetector([jarvis, hey], 50);
    expect(d.ready).toBe(true);
    expect(d.phrases).toEqual(["Джарвис", "Эй, Джарвис"]);
    expect(d.detect(word(1, 64, 71, 0.4, 2), 10_000)?.phrase).toBe("Джарвис");
    expect(d.detect(word(2.3, 74, 72, 0.4, -2), 20_000)?.phrase).toBe("Эй, Джарвис");
    expect(d.detect(word(0.4, 60, 73), 30_000)).toBeNull();
  });

  it("shares one cooldown and skips phrases without enough samples", () => {
    const d = new WakeDetector([jarvis, { phrase: "Jarvis", samples: [word(1, 60, 81)] }], 50);
    expect(d.phrases).toEqual(["Джарвис"]);
    expect(d.detect(word(1, 60, 82), 10_000)).not.toBeNull();
    expect(d.detect(word(1, 60, 83), 11_000)).toBeNull();
    expect(new WakeDetector([], 50).ready).toBe(false);
  });
});
