/**
 * MFCC feature extraction for the on-device wake-word spotter (ADR-0017).
 * 16 kHz mono, 25 ms frames (400 samples), 10 ms hop, 512-point FFT,
 * 26 mel bands (60–7600 Hz), 13 cepstral coefficients. Runs inside the
 * AudioWorklet (no DOM, no allocation per frame) and in unit tests.
 */

export const SAMPLE_RATE = 16000;
export const FRAME_LEN = 400;
export const HOP_LEN = 160;
export const FFT_SIZE = 512;
export const MEL_BANDS = 26;
export const NUM_CEPS = 13;

function hzToMel(hz: number): number {
  return 2595 * Math.log10(1 + hz / 700);
}

function melToHz(mel: number): number {
  return 700 * (10 ** (mel / 2595) - 1);
}

interface MelFilter {
  start: number;
  weights: Float32Array;
}

/** Triangular mel filterbank over FFT bins 0..FFT_SIZE/2. */
export function melFilterbank(bands = MEL_BANDS, fftSize = FFT_SIZE, rate = SAMPLE_RATE, lowHz = 60, highHz = 7600): MelFilter[] {
  const lo = hzToMel(lowHz);
  const hi = hzToMel(highHz);
  const points = Array.from({ length: bands + 2 }, (_, i) => melToHz(lo + ((hi - lo) * i) / (bands + 1)));
  const bin = points.map((hz) => Math.floor(((fftSize + 1) * hz) / rate));
  const filters: MelFilter[] = [];
  for (let b = 1; b <= bands; b++) {
    const left = bin[b - 1] ?? 0;
    const center = bin[b] ?? 0;
    const right = bin[b + 1] ?? 0;
    const weights = new Float32Array(Math.max(1, right - left + 1));
    for (let k = left; k <= right; k++) {
      let w = 0;
      if (k < center && center > left) w = (k - left) / (center - left);
      else if (k >= center && right > center) w = (right - k) / (right - center);
      else if (k === center) w = 1;
      weights[k - left] = Math.max(0, w);
    }
    filters.push({ start: left, weights });
  }
  return filters;
}

/** In-place iterative radix-2 complex FFT. `re`/`im` length must be a power of two. */
export function fft(re: Float32Array, im: Float32Array): void {
  const n = re.length;
  for (let i = 1, j = 0; i < n; i++) {
    let bit = n >> 1;
    for (; j & bit; bit >>= 1) j ^= bit;
    j ^= bit;
    if (i < j) {
      const tr = re[i] ?? 0;
      re[i] = re[j] ?? 0;
      re[j] = tr;
      const ti = im[i] ?? 0;
      im[i] = im[j] ?? 0;
      im[j] = ti;
    }
  }
  for (let len = 2; len <= n; len <<= 1) {
    const ang = (-2 * Math.PI) / len;
    const wr = Math.cos(ang);
    const wi = Math.sin(ang);
    for (let i = 0; i < n; i += len) {
      let cr = 1;
      let ci = 0;
      for (let k = 0; k < len / 2; k++) {
        const a = i + k;
        const b = a + len / 2;
        const xr = (re[b] ?? 0) * cr - (im[b] ?? 0) * ci;
        const xi = (re[b] ?? 0) * ci + (im[b] ?? 0) * cr;
        re[b] = (re[a] ?? 0) - xr;
        im[b] = (im[a] ?? 0) - xi;
        re[a] = (re[a] ?? 0) + xr;
        im[a] = (im[a] ?? 0) + xi;
        const ncr = cr * wr - ci * wi;
        ci = cr * wi + ci * wr;
        cr = ncr;
      }
    }
  }
}

/** Reusable extractor (pre-computed window, filterbank and DCT). */
export class MfccExtractor {
  private readonly window = new Float32Array(FRAME_LEN);
  private readonly filters = melFilterbank();
  private readonly dct: Float32Array[];
  private readonly re = new Float32Array(FFT_SIZE);
  private readonly im = new Float32Array(FFT_SIZE);
  private readonly power = new Float32Array(FFT_SIZE / 2 + 1);
  private readonly mel = new Float32Array(MEL_BANDS);

  constructor() {
    for (let i = 0; i < FRAME_LEN; i++) this.window[i] = 0.54 - 0.46 * Math.cos((2 * Math.PI * i) / (FRAME_LEN - 1));
    this.dct = Array.from({ length: NUM_CEPS }, (_, c) => {
      const row = new Float32Array(MEL_BANDS);
      for (let m = 0; m < MEL_BANDS; m++) row[m] = Math.cos((Math.PI * c * (m + 0.5)) / MEL_BANDS) * Math.sqrt((c === 0 ? 1 : 2) / MEL_BANDS);
      return row;
    });
  }

  /** MFCCs of one frame (`FRAME_LEN` samples in −1..1) written into `out` (length `NUM_CEPS`). */
  compute(frame: Float32Array, out: Float32Array = new Float32Array(NUM_CEPS)): Float32Array {
    const { re, im, power, mel } = this;
    re.fill(0);
    im.fill(0);
    let prev = 0;
    for (let i = 0; i < FRAME_LEN; i++) {
      const x = frame[i] ?? 0;
      re[i] = (x - 0.97 * prev) * (this.window[i] ?? 0); // pre-emphasis + Hamming
      prev = x;
    }
    fft(re, im);
    for (let k = 0; k < power.length; k++) {
      const r = re[k] ?? 0;
      const m = im[k] ?? 0;
      power[k] = (r * r + m * m) / FFT_SIZE;
    }
    for (let b = 0; b < MEL_BANDS; b++) {
      const f = this.filters[b];
      let e = 0;
      if (f) for (let k = 0; k < f.weights.length; k++) e += (power[f.start + k] ?? 0) * (f.weights[k] ?? 0);
      mel[b] = Math.log(e + 1e-10);
    }
    for (let c = 0; c < NUM_CEPS; c++) {
      const row = this.dct[c];
      let s = 0;
      if (row) for (let m = 0; m < MEL_BANDS; m++) s += (row[m] ?? 0) * (mel[m] ?? 0);
      out[c] = s;
    }
    return out;
  }
}

/** Frame energy in dBFS. */
export function frameEnergyDb(frame: Float32Array): number {
  let s = 0;
  for (let i = 0; i < frame.length; i++) {
    const x = frame[i] ?? 0;
    s += x * x;
  }
  return 10 * Math.log10(s / Math.max(1, frame.length) + 1e-12);
}
