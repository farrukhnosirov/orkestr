/**
 * Procedural sound recipes (§33 Sound Engine, §29 "sounds" in the theme).
 * No audio files: every cue is a small list of oscillator tones and noise
 * bursts that `engine.ts` renders with WebAudio. Pure data, unit-tested.
 */
import type { SoundPreset } from "@orkestr/protocol";

export type SoundCue =
  | "wake"
  | "listenStart"
  | "listenEnd"
  | "start"
  | "completion"
  | "error"
  | "approval"
  | "deploy"
  | "alert";

export const SOUND_CUES: readonly SoundCue[] = [
  "wake",
  "listenStart",
  "listenEnd",
  "start",
  "completion",
  "error",
  "approval",
  "deploy",
  "alert",
];

/** A concrete preset (Auto resolved against the theme). */
export type ResolvedPreset = Exclude<SoundPreset, "Auto">;
export const RESOLVED_PRESETS: readonly ResolvedPreset[] = ["Nebula", "Crystal", "Pulse", "Minimal"];

export type Wave = "sine" | "triangle" | "square" | "sawtooth";

export interface ToneSpec {
  wave: Wave;
  /** Start frequency, Hz. */
  freq: number;
  /** Optional glide target, Hz. */
  freqEnd?: number;
  /** Start offset, s. */
  at: number;
  /** Duration incl. release, s. */
  dur: number;
  /** Peak gain 0..1 (before master volume). */
  gain: number;
  /** Attack, s. */
  attack: number;
  /** Detune in cents. */
  detune?: number;
}

export interface NoiseSpec {
  at: number;
  dur: number;
  gain: number;
  /** Band-pass center, Hz. */
  filterHz: number;
}

export interface SoundRecipe {
  tones: ToneSpec[];
  noise?: NoiseSpec;
  /** Low-pass cutoff of the whole cue, Hz. */
  lowpassHz: number;
  /** Reverb send 0..1 (only rendered in full quality). */
  reverb: number;
}

/** Rendering quality, from the performance mode (§31). */
export type SoundQuality = "full" | "lite";

/** Equal-tempered note → Hz (`A4` = 440). */
export function noteHz(semitonesFromA4: number): number {
  return 440 * 2 ** (semitonesFromA4 / 12);
}

// Semitone offsets from A4.
const C5 = 3;
const D5 = 5;
const E5 = 7;
const G5 = 10;
const A5 = 12;
const B5 = 14;
const C6 = 15;
const G4 = -2;
const D4 = -7;
const Bb3 = -11;
const A3 = -12;

/** Melodic skeleton of each cue: [semitone, at, dur, relative gain]. */
const MELODY: Record<SoundCue, ReadonlyArray<readonly [number, number, number, number]>> = {
  wake: [
    [E5, 0, 0.22, 0.8],
    [B5, 0.09, 0.34, 1],
  ],
  listenStart: [
    [C5, 0, 0.09, 0.7],
    [G5, 0.05, 0.14, 0.8],
  ],
  listenEnd: [
    [G5, 0, 0.09, 0.7],
    [C5, 0.05, 0.16, 0.7],
  ],
  start: [
    [C5, 0, 0.18, 0.7],
    [E5, 0.07, 0.18, 0.75],
    [G5, 0.14, 0.3, 0.8],
  ],
  completion: [
    [C5, 0, 0.2, 0.75],
    [E5, 0.09, 0.2, 0.8],
    [G5, 0.18, 0.22, 0.85],
    [C6, 0.27, 0.55, 1],
  ],
  error: [
    [D4, 0, 0.22, 1],
    [Bb3, 0.16, 0.42, 1],
  ],
  approval: [
    [G4, 0, 0.18, 0.8],
    [D5, 0.12, 0.32, 0.9],
  ],
  deploy: [
    [A3, 0, 0.5, 0.6],
    [A5, 0.32, 0.4, 0.8],
  ],
  alert: [
    [A5, 0, 0.12, 1],
    [A5, 0.18, 0.12, 1],
    [A5, 0.36, 0.2, 1],
  ],
};

/** Cues that glide (frequency sweep) instead of stepping. */
const GLIDE: Partial<Record<SoundCue, number>> = { deploy: 2, listenStart: 1.12, listenEnd: 0.9 };

interface PresetVoice {
  waves: readonly Wave[];
  gain: number;
  attack: number;
  durScale: number;
  detune: number;
  lowpassHz: number;
  reverb: number;
  /** Add an octave partial (bell-like). */
  partial: number;
}

const PRESETS: Record<ResolvedPreset, PresetVoice> = {
  Nebula: { waves: ["sine", "triangle"], gain: 0.32, attack: 0.012, durScale: 1.25, detune: 7, lowpassHz: 5200, reverb: 0.35, partial: 0 },
  Crystal: { waves: ["sine"], gain: 0.3, attack: 0.004, durScale: 1, detune: 0, lowpassHz: 9000, reverb: 0.25, partial: 0.28 },
  Pulse: { waves: ["square", "sawtooth"], gain: 0.16, attack: 0.006, durScale: 0.85, detune: 4, lowpassHz: 2600, reverb: 0.15, partial: 0 },
  Minimal: { waves: ["sine"], gain: 0.22, attack: 0.004, durScale: 0.6, detune: 0, lowpassHz: 6000, reverb: 0, partial: 0 },
};

/** Builds the recipe of a cue for a preset and quality. */
export function recipeFor(cue: SoundCue, preset: ResolvedPreset, quality: SoundQuality = "full"): SoundRecipe {
  const p = PRESETS[preset];
  const glide = GLIDE[cue];
  // Error and alert keep a harsher edge in every preset so they stay recognizable.
  const urgent = cue === "error" || cue === "alert";
  const waves: readonly Wave[] = quality === "lite" ? [p.waves[0] ?? "sine"] : p.waves;
  const tones: ToneSpec[] = [];
  for (const [semi, at, dur, rel] of MELODY[cue]) {
    const freq = noteHz(semi);
    waves.forEach((wave, i) => {
      tones.push({
        wave: urgent && wave === "sine" ? "triangle" : wave,
        freq,
        ...(glide ? { freqEnd: freq * glide } : {}),
        at,
        dur: dur * p.durScale,
        gain: (p.gain * rel) / waves.length,
        attack: p.attack,
        ...(p.detune && i > 0 ? { detune: p.detune } : {}),
      });
    });
    if (p.partial > 0 && quality === "full") {
      tones.push({ wave: "sine", freq: freq * 2, at, dur: dur * p.durScale * 0.6, gain: p.gain * rel * p.partial, attack: p.attack });
    }
  }
  const recipe: SoundRecipe = {
    tones,
    lowpassHz: p.lowpassHz,
    reverb: quality === "full" ? p.reverb : 0,
  };
  if (cue === "deploy" && quality === "full") recipe.noise = { at: 0, dur: 0.6, gain: p.gain * 0.5, filterHz: 1800 };
  return recipe;
}

/** Total length of a recipe, s. */
export function recipeDuration(r: SoundRecipe): number {
  const ends = r.tones.map((t) => t.at + t.dur);
  if (r.noise) ends.push(r.noise.at + r.noise.dur);
  return Math.max(0, ...ends);
}

/** Theme → preset for `Auto` (unknown themes get a stable pick). */
export function presetForTheme(themeId: string): ResolvedPreset {
  switch (themeId) {
    case "nebula":
      return "Nebula";
    case "eclipse":
      return "Pulse";
    case "daylight":
      return "Crystal";
    default: {
      let h = 0;
      for (const ch of themeId) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
      return RESOLVED_PRESETS[h % RESOLVED_PRESETS.length] ?? "Nebula";
    }
  }
}

export function resolvePreset(preset: SoundPreset, themeId: string): ResolvedPreset {
  return preset === "Auto" ? presetForTheme(themeId) : preset;
}
