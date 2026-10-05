/**
 * Dynamic time warping over MFCC sequences (wake-word template matching,
 * ADR-0017 on-device spotter). Features are cepstral-mean-normalized and
 * c0 (overall loudness) is ignored, so microphone gain does not matter.
 * Pure, unit-tested on synthetic features.
 */

/** A feature sequence: frames × coefficients. */
export type FeatureSeq = readonly (ArrayLike<number>)[];

/** Cepstral mean normalization (drops c0). */
export function normalizeFeatures(seq: FeatureSeq, frames = seq.length): number[][] {
  const n = Math.min(frames, seq.length);
  if (n === 0) return [];
  const dims = (seq[0]?.length ?? 1) - 1;
  const mean = new Array<number>(dims).fill(0);
  for (let i = 0; i < n; i++) for (let d = 0; d < dims; d++) mean[d] = (mean[d] ?? 0) + (seq[i]?.[d + 1] ?? 0) / n;
  const out: number[][] = [];
  for (let i = 0; i < n; i++) {
    const row = new Array<number>(dims);
    for (let d = 0; d < dims; d++) row[d] = (seq[i]?.[d + 1] ?? 0) - (mean[d] ?? 0);
    out.push(row);
  }
  return out;
}

function dist(a: readonly number[], b: readonly number[]): number {
  let s = 0;
  for (let d = 0; d < a.length; d++) {
    const x = (a[d] ?? 0) - (b[d] ?? 0);
    s += x * x;
  }
  return Math.sqrt(s);
}

/**
 * DTW distance between `template` and the start of `input`, normalized by
 * path length. With `openEnd`, the input may continue after the matched
 * part ("Jarvis, what's up…"): the best end column in [0.6·T, 1.6·T] wins.
 * A Sakoe–Chiba band limits warping to ±40% of the template length.
 */
export function dtwDistance(template: readonly number[][], input: readonly number[][], openEnd = false): number {
  const T = template.length;
  const M = openEnd ? Math.min(input.length, Math.ceil(T * 1.6)) : input.length;
  if (T === 0 || M === 0) return Number.POSITIVE_INFINITY;
  const band = Math.max(Math.ceil(T * 0.4), Math.abs(T - M) + 1, 3);
  let prev = new Float64Array(M).fill(Number.POSITIVE_INFINITY);
  let cur = new Float64Array(M).fill(Number.POSITIVE_INFINITY);
  for (let i = 0; i < T; i++) {
    cur.fill(Number.POSITIVE_INFINITY);
    const center = openEnd ? i : Math.round((i * (M - 1)) / Math.max(1, T - 1));
    const lo = Math.max(0, center - band);
    const hi = Math.min(M - 1, center + band);
    const ti = template[i] ?? [];
    for (let j = lo; j <= hi; j++) {
      const c = dist(ti, input[j] ?? []);
      let best: number;
      if (i === 0 && j === 0) best = 0;
      else {
        const up = i > 0 ? (prev[j] ?? Infinity) : Infinity;
        const left = j > 0 ? (cur[j - 1] ?? Infinity) : Infinity;
        const diag = i > 0 && j > 0 ? (prev[j - 1] ?? Infinity) : Infinity;
        best = Math.min(up, left, diag);
      }
      cur[j] = c + best;
    }
    [prev, cur] = [cur, prev];
  }
  if (!openEnd) return (prev[M - 1] ?? Infinity) / (T + M);
  let best = Number.POSITIVE_INFINITY;
  for (let j = Math.floor(T * 0.6); j < M; j++) best = Math.min(best, (prev[j] ?? Infinity) / (T + j + 1));
  return best;
}

/** Mean pairwise DTW distance between enrollment templates (intra-class spread). */
export function meanPairwiseDistance(templates: readonly number[][][]): number {
  let sum = 0;
  let n = 0;
  for (let a = 0; a < templates.length; a++) {
    for (let b = a + 1; b < templates.length; b++) {
      const ta = templates[a];
      const tb = templates[b];
      if (!ta || !tb) continue;
      sum += dtwDistance(ta, tb);
      n++;
    }
  }
  return n ? sum / n : Number.NaN;
}

/**
 * Acceptance threshold from the enrollment spread and a 0..100 sensitivity:
 * sensitivity 0 accepts up to 1.1× the typical distance between the user's
 * own samples, 100 up to 1.9×.
 */
export function thresholdFor(intraDistance: number, sensitivity: number): number {
  const s = Math.min(100, Math.max(0, sensitivity)) / 100;
  return intraDistance * (1.1 + 0.8 * s);
}
