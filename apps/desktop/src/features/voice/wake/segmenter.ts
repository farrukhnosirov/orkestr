/**
 * Turns a stream of per-frame (energy, MFCC) into speech segments for the
 * wake-word spotter. Runs inside the AudioWorklet: keeps a short pre-roll
 * (VAD onset lags speech by a few frames), emits a segment at the end of
 * speech or early once `maxFrames` are collected (so "Jarvis, status…"
 * spoken without a pause still triggers). Pure, unit-tested.
 */
import { NUM_CEPS } from "./mfcc";
import { DEFAULT_VAD, EnergyVad, type VadOptions } from "./vad";

export interface Segment {
  /** frames × NUM_CEPS, flattened. */
  data: Float32Array;
  frames: number;
  /** Emitted before the end of speech (utterance continues). */
  early: boolean;
  /** Peak frame energy, dBFS. */
  peakDb: number;
}

export interface SegmenterOptions {
  minFrames: number;
  maxFrames: number;
  prerollFrames: number;
  vad: VadOptions;
}

export const DEFAULT_SEGMENTER: SegmenterOptions = { minFrames: 25, maxFrames: 160, prerollFrames: 8, vad: DEFAULT_VAD };

export class FrameSegmenter {
  private readonly vad: EnergyVad;
  private readonly preroll: Float32Array[] = [];
  private current: Float32Array[] = [];
  private peak = -120;
  private emittedEarly = false;

  constructor(private readonly opts: SegmenterOptions = DEFAULT_SEGMENTER) {
    this.vad = new EnergyVad(opts.vad);
  }

  get speaking(): boolean {
    return this.vad.speaking;
  }

  get noiseFloorDb(): number {
    return this.vad.noiseFloorDb;
  }

  /** Feeds one frame; returns a finished segment, if any. `mfcc` is copied. */
  push(energyDb: number, mfcc: ArrayLike<number>): Segment | null {
    const frame = Float32Array.from(mfcc);
    const ev = this.vad.push(energyDb);
    if (ev === "start") {
      this.current = [...this.preroll];
      this.peak = energyDb;
      this.emittedEarly = false;
    }
    if (this.vad.speaking || ev === "end") {
      if (!this.emittedEarly) this.current.push(frame);
      this.peak = Math.max(this.peak, energyDb);
      if (!this.emittedEarly && this.current.length >= this.opts.maxFrames) {
        this.emittedEarly = true;
        return this.flush(true);
      }
    }
    this.preroll.push(frame);
    if (this.preroll.length > this.opts.prerollFrames) this.preroll.shift();
    if (ev === "end") {
      if (this.emittedEarly) {
        this.current = [];
        return null;
      }
      // Drop the hangover tail (silence after the word).
      const keep = Math.max(0, this.current.length - Math.floor(this.opts.vad.hangoverFrames * 0.8));
      this.current = this.current.slice(0, keep);
      if (this.current.length >= this.opts.minFrames) return this.flush(false);
      this.current = [];
    }
    return null;
  }

  private flush(early: boolean): Segment {
    const frames = this.current.length;
    const data = new Float32Array(frames * NUM_CEPS);
    this.current.forEach((f, i) => data.set(f.subarray(0, NUM_CEPS), i * NUM_CEPS));
    const seg: Segment = { data, frames, early, peakDb: this.peak };
    this.current = [];
    return seg;
  }

  reset(): void {
    this.vad.reset();
    this.preroll.length = 0;
    this.current = [];
    this.emittedEarly = false;
  }
}

/** Unflattens segment data into frames. */
export function segmentFrames(seg: Pick<Segment, "data" | "frames">): number[][] {
  const out: number[][] = [];
  for (let i = 0; i < seg.frames; i++) out.push(Array.from(seg.data.subarray(i * NUM_CEPS, (i + 1) * NUM_CEPS)));
  return out;
}
