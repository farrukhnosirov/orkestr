/**
 * Wake-word enrollment (ADR-0017, local): the user says the phrase 3–5
 * times; each VAD segment is checked (length, loudness, consistency with
 * the previous samples) and kept as an MFCC template. Pure, unit-tested.
 */
import { normalizeFeatures } from "./dtw";
import type { Segment } from "./segmenter";
import { segmentFrames } from "./segmenter";
import { checkEnrollSample, consistentWith, type EnrollProblem, MAX_ENROLL_SAMPLES, MIN_ENROLL_SAMPLES } from "./spotter";

export type EnrollResult = { ok: true; count: number } | { ok: false; problem: EnrollProblem | "full" };

export class Enrollment {
  private readonly list: number[][][] = [];

  constructor(initial: readonly number[][][] = []) {
    for (const s of initial.slice(0, MAX_ENROLL_SAMPLES)) this.list.push(s);
  }

  get samples(): readonly number[][][] {
    return this.list;
  }

  get count(): number {
    return this.list.length;
  }

  /** Enough samples to listen for the wake word. */
  get complete(): boolean {
    return this.list.length >= MIN_ENROLL_SAMPLES;
  }

  get full(): boolean {
    return this.list.length >= MAX_ENROLL_SAMPLES;
  }

  /** Offers a VAD segment as a sample. */
  add(seg: Pick<Segment, "data" | "frames" | "peakDb" | "early">): EnrollResult {
    if (this.full) return { ok: false, problem: "full" };
    if (seg.early) return { ok: false, problem: "tooLong" };
    const problem = checkEnrollSample(seg.frames, seg.peakDb);
    if (problem) return { ok: false, problem };
    const frames = segmentFrames(seg);
    // Raw MFCCs are stored (the spotter normalizes); consistency is judged on normalized features.
    if (!consistentWith(normalizeFeatures(frames), this.list.map((x) => normalizeFeatures(x)))) return { ok: false, problem: "inconsistent" };
    this.list.push(frames);
    return { ok: true, count: this.list.length };
  }

  removeLast(): void {
    this.list.pop();
  }

  clear(): void {
    this.list.length = 0;
  }
}
