/**
 * Audio front end of the on-device voice pipeline (ADR-0017): resamples the
 * microphone to 16 kHz mono, frames it (25 ms / 10 ms hop), measures energy,
 * extracts MFCCs, runs the VAD segmenter and keeps CPU accounting. Pure (an
 * injectable clock), so it is unit-tested in Node; `worklet.ts` wraps it in
 * an `AudioWorkletProcessor` so all of this runs on the audio thread, never
 * on the UI thread.
 *
 * CPU budget: MFCC extraction is the dominant cost. When the measured load
 * exceeds the budget, quiet frames reuse the previous coefficients instead
 * of computing new ones (speech frames are always computed), which cuts the
 * idle cost to the energy computation alone.
 */
import { FRAME_LEN, frameEnergyDb, HOP_LEN, MfccExtractor, NUM_CEPS, SAMPLE_RATE } from "./mfcc";
import { DEFAULT_SEGMENTER, FrameSegmenter, type Segment, type SegmenterOptions } from "./segmenter";

/** Linear-interpolating resampler with a box pre-filter (anti-aliasing for integer-ish ratios). */
export class Resampler {
  private readonly ratio: number;
  private readonly box: number;
  private pos = 0;
  private prev = 0;
  private hist: number[] = [];

  constructor(
    readonly inRate: number,
    readonly outRate: number = SAMPLE_RATE,
  ) {
    this.ratio = inRate / outRate;
    this.box = Math.max(1, Math.round(this.ratio));
  }

  /** Converts a block; returns the produced 16 kHz samples. */
  process(input: Float32Array): Float32Array {
    if (this.ratio === 1) return input.slice();
    const out: number[] = [];
    for (let i = 0; i < input.length; i++) {
      // Box filter over the last `box` input samples.
      this.hist.push(input[i] ?? 0);
      if (this.hist.length > this.box) this.hist.shift();
      let s = 0;
      for (const v of this.hist) s += v;
      const cur = s / this.hist.length;
      // Emit every output sample whose position falls between prev and cur.
      while (this.pos <= 1) {
        out.push(this.prev + (cur - this.prev) * this.pos);
        this.pos += this.ratio;
      }
      this.pos -= 1;
      this.prev = cur;
    }
    return Float32Array.from(out);
  }
}

export interface FrontEndOptions {
  inputRate: number;
  segmenter?: SegmenterOptions;
  /** Average CPU budget in percent of one core. */
  cpuBudgetPct?: number;
  /** Clock in ms (worklet: `performance.now` or `Date.now`). */
  now?: () => number;
  /** Emit a level report every N frames (10 ms each). */
  levelEvery?: number;
}

export interface LevelReport {
  /** Peak frame energy over the report window, dBFS. */
  db: number;
  speaking: boolean;
  floorDb: number;
}

export interface FrontEndOutput {
  segments: Segment[];
  levels: LevelReport[];
  /** 16 kHz PCM produced by this block (only when recording). */
  pcm: Float32Array | null;
}

export class AudioFrontEnd {
  private readonly resampler: Resampler;
  private readonly mfcc = new MfccExtractor();
  private readonly segmenter: FrameSegmenter;
  private readonly frame = new Float32Array(FRAME_LEN);
  private readonly ceps = new Float32Array(NUM_CEPS);
  private buf: Float32Array = new Float32Array(0);
  private levelPeak = -120;
  private levelCount = 0;
  private readonly levelEvery: number;
  private readonly now: () => number;
  private budgetPct: number;
  /** Work and audio time in the current accounting window (ms). */
  private workMs = 0;
  private audioMs = 0;
  /** Smoothed load in percent of one core. */
  loadPct = 0;
  /** Whether quiet frames reuse coefficients (over budget). */
  economy = false;
  recording = false;
  /** Wake spotting on/off (level metering keeps running). */
  segmenting = true;

  constructor(opts: FrontEndOptions) {
    this.resampler = new Resampler(opts.inputRate);
    this.segmenter = new FrameSegmenter(opts.segmenter ?? DEFAULT_SEGMENTER);
    this.levelEvery = opts.levelEvery ?? 5;
    this.now = opts.now ?? (() => Date.now());
    this.budgetPct = opts.cpuBudgetPct ?? 2;
  }

  setBudget(pct: number): void {
    this.budgetPct = Math.min(10, Math.max(1, pct));
  }

  /** Processes one block of input samples (any rate, mono). */
  process(input: Float32Array): FrontEndOutput {
    const t0 = this.now();
    const out: FrontEndOutput = { segments: [], levels: [], pcm: null };
    const pcm = this.resampler.process(input);
    if (this.recording) out.pcm = pcm;
    // Append to the framing buffer.
    const merged = new Float32Array(this.buf.length + pcm.length);
    merged.set(this.buf);
    merged.set(pcm, this.buf.length);
    let off = 0;
    while (merged.length - off >= FRAME_LEN) {
      this.frame.set(merged.subarray(off, off + FRAME_LEN));
      off += HOP_LEN;
      const db = frameEnergyDb(this.frame);
      this.levelPeak = Math.max(this.levelPeak, db);
      if (++this.levelCount >= this.levelEvery) {
        out.levels.push({ db: this.levelPeak, speaking: this.segmenter.speaking, floorDb: this.segmenter.noiseFloorDb });
        this.levelPeak = -120;
        this.levelCount = 0;
      }
      if (!this.segmenting) continue;
      const quiet = !this.segmenter.speaking && db < this.segmenter.noiseFloorDb + 6;
      if (!(this.economy && quiet)) this.mfcc.compute(this.frame, this.ceps);
      const seg = this.segmenter.push(db, this.ceps);
      if (seg) out.segments.push(seg);
    }
    this.buf = merged.slice(off);
    this.account(this.now() - t0, (input.length / this.resampler.inRate) * 1000);
    return out;
  }

  private account(work: number, audio: number): void {
    this.workMs += Math.max(0, work);
    this.audioMs += audio;
    if (this.audioMs < 1000) return;
    const pct = (this.workMs / this.audioMs) * 100;
    this.loadPct = this.loadPct === 0 ? pct : this.loadPct * 0.7 + pct * 0.3;
    // Hysteresis: enter economy above budget, leave below 60 % of it.
    if (this.loadPct > this.budgetPct) this.economy = true;
    else if (this.loadPct < this.budgetPct * 0.6) this.economy = false;
    this.workMs = 0;
    this.audioMs = 0;
  }

  reset(): void {
    this.segmenter.reset();
    this.buf = new Float32Array(0);
  }
}
