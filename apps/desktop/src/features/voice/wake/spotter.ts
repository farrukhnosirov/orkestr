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

/** A spotted wake phrase: which phrase's samples matched best. */
export interface WakeDetection extends WakeHit {
  phrase: string;
}

/**
 * Several wake phrases (ADR-0056): one {@link WakeSpotter} per recorded
 * phrase, a shared cooldown, the best match (distance relative to that
 * phrase's own threshold) wins.
 */
export class WakeDetector {
  private readonly spotters: { phrase: string; spotter: WakeSpotter }[];
  private lastHit = -Infinity;

  constructor(sets: readonly StoredTemplates[], sensitivity: number) {
    this.spotters = sets
      .filter((s) => s.samples.length >= MIN_ENROLL_SAMPLES)
      .map((s) => ({ phrase: s.phrase, spotter: new WakeSpotter(s.samples, sensitivity) }))
      .filter((x) => x.spotter.ready);
  }

  get ready(): boolean {
    return this.spotters.length > 0;
  }

  get phrases(): string[] {
    return this.spotters.map((s) => s.phrase);
  }

  /** The closest phrase (distance relative to its own threshold; ≤ 1 = accepted), no cooldown. */
  closest(segment: readonly number[][], early = false): { phrase: string; distance: number; threshold: number; ratio: number } | null {
    let best: { phrase: string; distance: number; threshold: number; ratio: number } | null = null;
    for (const { phrase, spotter } of this.spotters) {
      const distance = spotter.distance(segment, early);
      const threshold = spotter.acceptThreshold;
      const ratio = distance / threshold;
      if (Number.isFinite(ratio) && (!best || ratio < best.ratio)) best = { phrase, distance, threshold, ratio };
    }
    return best;
  }

  /** Detects any wake phrase in a segment (respects a shared cooldown). */
  detect(segment: readonly number[][], now: number, early = false): WakeDetection | null {
    if (!this.ready || now - this.lastHit < WAKE_COOLDOWN_MS) return null;
    const best = this.closest(segment, early);
    if (!best || !(best.distance <= best.threshold)) return null;
    this.lastHit = now;
    const { distance, threshold, phrase } = best;
    return { phrase, distance, threshold, score: Math.max(0, Math.min(1, 1 - distance / (threshold * 2))) };
  }
}

// ---------------------------------------------------------------------------
// Local persistence (this device only).
// ---------------------------------------------------------------------------

/** One phrase's samples (v1 stored a single set under {@link TEMPLATES_KEY}). */
export const TEMPLATES_KEY = "orkestr.voice.wakeTemplates.v1";
/** Every phrase's samples (ADR-0056): `{ sets: StoredTemplates[] }`. */
export const TEMPLATE_SETS_KEY = "orkestr.voice.wakeTemplates.v2";

export interface StoredTemplates {
  phrase: string;
  /** samples × frames × coefficients, rounded. */
  samples: number[][][];
}

function parseSet(v: unknown): StoredTemplates | null {
  if (!v || typeof v !== "object") return null;
  const o = v as { phrase?: unknown; samples?: unknown };
  if (typeof o.phrase !== "string" || !Array.isArray(o.samples)) return null;
  const samples = o.samples.filter(
    (s): s is number[][] => Array.isArray(s) && s.every((f) => Array.isArray(f) && f.every((x) => typeof x === "number" && Number.isFinite(x))),
  );
  return { phrase: o.phrase, samples };
}

function sameKey(a: string, b: string): boolean {
  const k = (p: string) => p.trim().toLowerCase().replace(/ё/g, "е").replace(/[.,!?;:«»"']/g, "").replace(/\s+/g, " ");
  return k(a) === k(b);
}

/** Every stored phrase's samples (the v1 single set when no v2 list exists). */
export function loadTemplateSets(): StoredTemplates[] {
  try {
    const raw = window.localStorage.getItem(TEMPLATE_SETS_KEY);
    if (raw) {
      const v: unknown = JSON.parse(raw);
      const sets = (v as { sets?: unknown } | null)?.sets;
      if (Array.isArray(sets)) return sets.map(parseSet).filter((s): s is StoredTemplates => s !== null && s.samples.length > 0);
    }
    const legacy = window.localStorage.getItem(TEMPLATES_KEY);
    if (!legacy) return [];
    const one = parseSet(JSON.parse(legacy));
    return one && one.samples.length > 0 ? [one] : [];
  } catch {
    return [];
  }
}

/** Replaces every stored set (an empty list removes the samples). */
export function saveTemplateSets(sets: readonly StoredTemplates[]): void {
  try {
    const kept = sets.filter((s) => s.samples.length > 0);
    window.localStorage.removeItem(TEMPLATES_KEY); // migrated to v2
    if (kept.length === 0) window.localStorage.removeItem(TEMPLATE_SETS_KEY);
    else {
      const rounded = kept.map((t) => ({ phrase: t.phrase, samples: t.samples.map((s) => s.map((f) => f.map((x) => Math.round(x * 1000) / 1000))) }));
      window.localStorage.setItem(TEMPLATE_SETS_KEY, JSON.stringify({ sets: rounded }));
    }
  } catch {
    /* storage unavailable: enrollment lasts for this session only */
  }
}

/** The samples of one phrase, if recorded. */
export function templatesFor(phrase: string, sets: readonly StoredTemplates[] = loadTemplateSets()): StoredTemplates | null {
  return sets.find((s) => sameKey(s.phrase, phrase)) ?? null;
}

/** Stores (or with `null` removes) one phrase's samples, keeping the others. */
export function saveTemplatesFor(phrase: string, samples: number[][][] | null): void {
  const others = loadTemplateSets().filter((s) => !sameKey(s.phrase, phrase));
  saveTemplateSets(samples && samples.length > 0 ? [...others, { phrase, samples }] : others);
}

/** Keeps only the samples of phrases still in the list (a removed phrase forgets its voice). */
export function pruneTemplateSets(phrases: readonly string[]): void {
  const sets = loadTemplateSets();
  const kept = sets.filter((s) => phrases.some((p) => sameKey(p, s.phrase)));
  if (kept.length !== sets.length) saveTemplateSets(kept);
}

/** Total recorded samples over every phrase (wake listener restart key). */
export function templatesSignature(sets: readonly StoredTemplates[] = loadTemplateSets()): string {
  return sets.map((s) => `${s.phrase.toLowerCase()}:${s.samples.length}`).join(",");
}

/** First stored set (compatibility with single-phrase callers). */
export function loadTemplates(): StoredTemplates | null {
  return loadTemplateSets()[0] ?? null;
}

/** `null` removes every phrase's samples; a set replaces that phrase's samples. */
export function saveTemplates(t: StoredTemplates | null): void {
  if (!t) saveTemplateSets([]);
  else saveTemplatesFor(t.phrase, t.samples);
}
