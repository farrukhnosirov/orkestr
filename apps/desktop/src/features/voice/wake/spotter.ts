/**
 * Wake-word spotter: DTW template matching of speech segments against the
 * user's enrollment samples (ADR-0017, on-device; nothing leaves the
 * machine). Also the enrollment rules and template persistence (local to
 * this device — templates are derived from the user's voice and stay in the
 * webview's storage, never in the core or the cloud). Pure except storage.
 */
import { dtwDistance, meanPairwiseDistance, normalizeFeatures, thresholdFor } from "./dtw";

export const MIN_ENROLL_SAMPLES = 3;
export const MAX_ENROLL_SAMPLES = 5;
/** Enrollment samples: 0.3–1.6 s of speech. */
export const ENROLL_MIN_FRAMES = 30;
export const ENROLL_MAX_FRAMES = 160;
/** No second wake within this time. */
export const WAKE_COOLDOWN_MS = 2500;

export interface WakeHit {
  distance: number;
  threshold: number;
  /** 0..1, higher = more confident. */
  score: number;
}

export type EnrollProblem = "tooShort" | "tooLong" | "tooQuiet" | "inconsistent";

/** Checks one enrollment sample. */
export function checkEnrollSample(frames: number, peakDb: number): EnrollProblem | null {
  if (frames < ENROLL_MIN_FRAMES) return "tooShort";
  if (frames > ENROLL_MAX_FRAMES) return "tooLong";
  if (peakDb < -45) return "tooQuiet";
  return null;
}

/**
 * Checks a new sample against already accepted ones: rejects a sample whose
 * distance to the others is far above their spread (a different word or a
 * cough), once at least two samples exist.
 */
export function consistentWith(sample: number[][], accepted: readonly number[][][]): boolean {
  if (accepted.length < 2) return true;
  const spread = meanPairwiseDistance(accepted);
  const d = accepted.reduce((s, t) => s + dtwDistance(t, sample), 0) / accepted.length;
  return d <= spread * 2.2;
}

export class WakeSpotter {
  private readonly templates: number[][][];
  private readonly threshold: number;
  private lastHit = -Infinity;

  /** `raw` are un-normalized MFCC frames per enrollment sample. */
  constructor(raw: readonly (readonly number[][])[], sensitivity: number) {
    this.templates = raw.map((t) => normalizeFeatures(t));
    const spread = meanPairwiseDistance(this.templates);
    this.threshold = thresholdFor(Number.isFinite(spread) ? spread : 0, sensitivity);
  }

  get ready(): boolean {
    return this.templates.length >= MIN_ENROLL_SAMPLES && this.threshold > 0;
  }

  get acceptThreshold(): number {
    return this.threshold;
  }

  /** Best (lowest) distance of a segment to any template. */
  distance(segment: readonly number[][], early = false): number {
    let best = Number.POSITIVE_INFINITY;
    for (const t of this.templates) {
      // CMN over roughly the word-length prefix when the utterance continues.
      const input = normalizeFeatures(segment, early ? Math.ceil(t.length * 1.3) : segment.length);
      best = Math.min(best, dtwDistance(t, input, early || segment.length > t.length * 1.6));
    }
    return best;
  }

  /** Detects the wake word in a segment (respects a cooldown). */
  detect(segment: readonly number[][], now: number, early = false): WakeHit | null {
    if (!this.ready || now - this.lastHit < WAKE_COOLDOWN_MS) return null;
    const distance = this.distance(segment, early);
    if (!(distance <= this.threshold)) return null;
    this.lastHit = now;
    return { distance, threshold: this.threshold, score: Math.max(0, Math.min(1, 1 - distance / (this.threshold * 2))) };
  }
}

// ---------------------------------------------------------------------------
// Local persistence (this device only).
// ---------------------------------------------------------------------------

export const TEMPLATES_KEY = "orkestr.voice.wakeTemplates.v1";

export interface StoredTemplates {
  phrase: string;
  /** samples × frames × coefficients, rounded. */
  samples: number[][][];
}

export function loadTemplates(): StoredTemplates | null {
  try {
    const raw = window.localStorage.getItem(TEMPLATES_KEY);
    if (!raw) return null;
    const v: unknown = JSON.parse(raw);
    if (!v || typeof v !== "object") return null;
    const o = v as { phrase?: unknown; samples?: unknown };
    if (typeof o.phrase !== "string" || !Array.isArray(o.samples)) return null;
    const samples = o.samples.filter(
      (s): s is number[][] => Array.isArray(s) && s.every((f) => Array.isArray(f) && f.every((x) => typeof x === "number" && Number.isFinite(x))),
    );
    return { phrase: o.phrase, samples };
  } catch {
    return null;
  }
}

export function saveTemplates(t: StoredTemplates | null): void {
  try {
    if (!t) window.localStorage.removeItem(TEMPLATES_KEY);
    else {
      const rounded = t.samples.map((s) => s.map((f) => f.map((x) => Math.round(x * 1000) / 1000)));
      window.localStorage.setItem(TEMPLATES_KEY, JSON.stringify({ phrase: t.phrase, samples: rounded }));
    }
  } catch {
    /* storage unavailable: enrollment lasts for this session only */
  }
}
