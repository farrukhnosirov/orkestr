/**
 * Jarvis voice visual — the pure model (no React, no canvas; unit-tested in
 * `jarvis.test.ts`): which visual state a voice phase shows, the per-frame
 * dynamics of the reactor (wake bloom, attack / decay of the level and the
 * spectrum bands, ring speeds, scanning sweep, error flicker), spectrum bands
 * from an `AnalyserNode`'s frequency data, a synthetic spectrum when there is
 * none, how much of the reply is typed out, and the frame-rate budget.
 *
 * The component API is generic so other voice modes plug in (`mode`:
 * command / chat / cursor): every host derives a {@link JarvisState} and feeds
 * level, transcript and reply — nothing here knows about the pipeline.
 */
import type { PerformanceMode } from "@orkestr/protocol";

/** What the reactor shows. */
export type JarvisState = "idle" | "wake" | "listening" | "thinking" | "speaking" | "done" | "error";

/** Which assistant mode is talking (accent / labels only). */
export type JarvisMode = "command" | "chat" | "cursor";

export const JARVIS_STATES: readonly JarvisState[] = ["idle", "wake", "listening", "thinking", "speaking", "done", "error"];

/** Reactor diameter range (Settings → Orb → voice circle size) and default, CSS px. */
export const JARVIS_SIZE_RANGE = [120, 180] as const;
export const JARVIS_DEFAULT_SIZE = 150;
export const JARVIS_SIZES = [120, 150, 180] as const;

/** Clamps a stored size to the range (garbage → default). */
export function clampJarvisSize(v: unknown): number {
  const n = typeof v === "number" && Number.isFinite(v) ? Math.round(v) : JARVIS_DEFAULT_SIZE;
  return Math.min(JARVIS_SIZE_RANGE[1], Math.max(JARVIS_SIZE_RANGE[0], n));
}

/** Duration of the wake bloom intro, ms. */
export const WAKE_INTRO_MS = 750;
/** Collapse-back-into-the-orb animation when the voice card closes, ms. */
export const COLLAPSE_MS = 520;

export interface PhaseInput {
  /** Voice pipeline phase (`idle`, `listening`, `transcribing`, `thinking`, `confirm`, `result`, `error`). */
  phase: string;
  /** TTS is speaking. */
  speaking: boolean;
  /** How the command started (`WakeWord`, `PushToTalk`, `Typed`, …). */
  source?: string | null;
  /** Milliseconds since listening started (wake bloom window). */
  sinceListenMs?: number;
}

/** The visual state for a pipeline phase. Speaking wins over the phase it speaks in. */
export function jarvisStateOf(p: PhaseInput): JarvisState {
  if (p.phase === "error") return "error";
  if (p.phase === "listening") {
    return p.source === "WakeWord" && (p.sinceListenMs ?? Infinity) < WAKE_INTRO_MS ? "wake" : "listening";
  }
  if (p.speaking) return "speaking";
  switch (p.phase) {
    case "transcribing":
    case "thinking":
      return "thinking";
    case "confirm":
      return "listening";
    case "result":
      return "done";
    default:
      return "idle";
  }
}

/** Whether a state is "on" (the card is open). */
export function isActive(s: JarvisState): boolean {
  return s !== "idle";
}

/** Status-line key (`jarvis:status.*`). */
export function statusKey(s: JarvisState, phase?: string): "status.transcribing" | "status.confirm" | "status.listening" | "status.thinking" | "status.speaking" | "status.done" | "status.error" | "status.idle" {
  if (phase === "transcribing") return "status.transcribing";
  if (phase === "confirm") return "status.confirm";
  switch (s) {
    case "wake":
    case "listening":
      return "status.listening";
    case "thinking":
      return "status.thinking";
    case "speaking":
      return "status.speaking";
    case "done":
      return "status.done";
    case "error":
      return "status.error";
    default:
      return "status.idle";
  }
}

// ---------------------------------------------------------------------------
// Frame budget.
// ---------------------------------------------------------------------------

/** Reactor frame rate: 60 fps by default, lower in Performance, a still frame in Battery Saver / reduced motion / hidden. */
export function reactorFps(mode: PerformanceMode, reducedMotion: boolean, hidden: boolean): number {
  if (hidden || reducedMotion) return 0;
  switch (mode) {
    case "UltraVisual":
    case "Balanced":
      return 60;
    case "Performance":
      return 24;
    case "BatterySaver":
      return 0;
  }
}

// ---------------------------------------------------------------------------
// Signal: level + spectrum.
// ---------------------------------------------------------------------------

/** Spectrum band count the renderer draws (mirrored around the circle). */
export const BANDS = 32;

export interface SignalSample {
  level: number;
  /** Spoken share of the reply 0..1 (null = unknown). */
  progress: number | null;
}

/** A live signal (spectrum + level + reply progress) the reactor samples once per frame. */
export interface JarvisSignal {
  /**
   * Fills `out` (length {@link BANDS}) with band energies 0..1 and returns the
   * level and progress, or `null` when nothing is live (the reactor synthesizes).
   */
  sample(out: Float32Array, now: number): SignalSample | null;
}

/**
 * Log-spaced voice bands (≈ 90 Hz … 5 kHz) from `getByteFrequencyData`
 * output, 0..1 each. `data` covers 0 … sampleRate / 2.
 */
export function bandsFromFrequency(data: ArrayLike<number>, out: Float32Array, sampleRate = 48_000, lowHz = 90, highHz = 5000): void {
  const bins = data.length;
  if (bins === 0) {
    out.fill(0);
    return;
  }
  const hzPerBin = sampleRate / 2 / bins;
  const n = out.length;
  for (let b = 0; b < n; b++) {
    const f0 = lowHz * Math.pow(highHz / lowHz, b / n);
    const f1 = lowHz * Math.pow(highHz / lowHz, (b + 1) / n);
    const i0 = Math.min(bins - 1, Math.max(0, Math.floor(f0 / hzPerBin)));
    const i1 = Math.min(bins - 1, Math.max(i0, Math.ceil(f1 / hzPerBin) - 1));
    let peak = 0;
    for (let i = i0; i <= i1; i++) peak = Math.max(peak, data[i] ?? 0);
    // Byte data is already in dB (minDecibels … maxDecibels → 0 … 255); cut the noise floor.
    const v = (peak - 70) / 150;
    // Higher bands carry less energy in speech: lift them a little.
    out[b] = Math.min(1, Math.max(0, v * (1 + (b / n) * 0.6)));
  }
}

/** Level 0..1 from a band set (mean of the louder half). */
export function levelOfBands(bands: Float32Array): number {
  const sorted = Array.from(bands).sort((a, b) => b - a);
  const top = sorted.slice(0, Math.max(1, Math.floor(sorted.length / 2)));
  return Math.min(1, top.reduce((s, v) => s + v, 0) / top.length);
}

/** Cheap deterministic pseudo-noise in −1..1. */
export function noise(x: number): number {
  const s = Math.sin(x * 12.9898) * 43758.5453;
  return (s - Math.floor(s)) * 2 - 1;
}

/**
 * A plausible voice spectrum from a level alone (the orb window between
 * channel frames, the system TTS voice, previews): a formant-shaped envelope
 * that wobbles with time.
 */
export function syntheticBands(level: number, t: number, out: Float32Array): void {
  const n = out.length;
  for (let b = 0; b < n; b++) {
    const x = b / n;
    const shape = 0.75 * Math.exp(-((x - 0.28) ** 2) / 0.018) + 0.55 * Math.exp(-((x - 0.62) ** 2) / 0.03) + 0.12;
    const wob = 0.6 + 0.4 * Math.sin(t * 7.3 + b * 0.9) * Math.sin(t * 3.1 + b * 0.37) + 0.15 * noise(b + Math.floor(t * 18));
    out[b] = Math.min(1, Math.max(0, level * shape * wob * 1.25));
  }
}

/** Exponential approach with separate attack / decay time constants (ms). */
export function follow(current: number, target: number, dtMs: number, attackMs: number, decayMs: number): number {
  const tau = target > current ? attackMs : decayMs;
  if (tau <= 0) return target;
  return current + (target - current) * (1 - Math.exp(-dtMs / tau));
}

// ---------------------------------------------------------------------------
// Reactor dynamics (per frame, deterministic for a given input sequence).
// ---------------------------------------------------------------------------

export interface ReactorFrame {
  state: JarvisState;
  /** Wake bloom progress 0..1 (1 = settled). */
  intro: number;
  /** Overall scale of the reactor (bloom, done settle). */
  scale: number;
  /** Brightness 0..1 (done dims, error flickers). */
  energy: number;
  /** Smoothed level 0..1. */
  level: number;
  /** Smoothed spectrum bands 0..1. */
  bands: Float32Array;
  /** Ring rotations, radians (outer ticks, segmented arcs, dashed ring, inner chaser). */
  rot: readonly [number, number, number, number];
  /** Scanning sweep angle (thinking), radians, and its visibility 0..1. */
  sweep: number;
  sweepAlpha: number;
  /** Speaking ripple phase 0..1 and its visibility. */
  ripple: number;
  rippleAlpha: number;
  /** Error tint 0..1. */
  alarm: number;
  /** Time, seconds. */
  t: number;
}

/** Base angular speeds (rad/s) of the four rings. */
const RING_SPEED = [0.12, -0.55, 0.95, -1.6] as const;

export class ReactorDynamics {
  private state: JarvisState = "idle";
  private stateAt = 0;
  private introStart = -Infinity;
  private level = 0;
  private energy = 0;
  private scale = 0.6;
  private alarm = 0;
  private sweepAlpha = 0;
  private rippleAlpha = 0;
  private speedMul = 1;
  private sweep = 0;
  private ripple = 0;
  private flicker = 1;
  private rot: [number, number, number, number] = [0, 0, 0, 0];
  private time = 0;
  readonly bands = new Float32Array(BANDS);
  private readonly synth = new Float32Array(BANDS);

  get current(): { state: JarvisState; since: number } {
    return { state: this.state, since: this.stateAt };
  }

  /** Advances by `dtMs`. `raw` = live bands (null → synthesized from `level`). */
  step(now: number, dtMs: number, state: JarvisState, level: number, raw: Float32Array | null): ReactorFrame {
    const dt = Math.max(0, Math.min(100, dtMs));
    if (state !== this.state) {
      // From rest into any active state: play the bloom (a wake always does).
      if (state === "wake" || ((this.state === "idle" || this.state === "done") && state !== "idle" && state !== "done")) this.introStart = now;
      this.state = state;
      this.stateAt = now;
    }
    this.time += dt / 1000;
    const intro = Math.min(1, Math.max(0, (now - this.introStart) / WAKE_INTRO_MS));

    const target = state === "idle" ? 0 : state === "thinking" ? 0.22 + 0.08 * Math.sin(this.time * 3) : state === "done" ? 0.06 : level;
    this.level = follow(this.level, Math.min(1, Math.max(0, target)), dt, 45, 260);

    syntheticBands(this.level, this.time, this.synth);
    const quiet = state === "thinking" || state === "done" || state === "idle";
    const src = raw && !quiet ? raw : this.synth;
    for (let i = 0; i < BANDS; i++) {
      const v = quiet ? (src[i] ?? 0) * 0.6 : (src[i] ?? 0);
      this.bands[i] = follow(this.bands[i] ?? 0, v, dt, 35, 220);
    }

    const wantEnergy = state === "idle" ? 0 : state === "done" ? 0.6 : 1;
    this.energy = follow(this.energy, wantEnergy, dt, 160, 420);
    const wantScale = state === "done" ? 0.9 : state === "idle" ? 0.6 : 1;
    this.scale = follow(this.scale, wantScale, dt, 220, 380);
    let scale = this.scale;
    if (intro < 1) {
      const bloom = 1 - Math.pow(1 - intro, 3);
      scale = Math.min(scale, 0.3 + 0.7 * bloom) + Math.sin(intro * Math.PI) * 0.07;
    }

    const wantMul = state === "thinking" ? 2.4 : state === "done" ? 0.35 : state === "idle" ? 0.2 : state === "error" ? 0.6 : 1 + this.level * 1.4;
    this.speedMul = follow(this.speedMul, wantMul, dt, 300, 600);
    for (let i = 0; i < 4; i++) this.rot[i] = ((this.rot[i] ?? 0) + (RING_SPEED[i] ?? 0) * this.speedMul * (dt / 1000)) % (Math.PI * 2);

    this.sweepAlpha = follow(this.sweepAlpha, state === "thinking" ? 1 : 0, dt, 200, 300);
    this.sweep = (this.sweep + 3.4 * (dt / 1000)) % (Math.PI * 2);
    this.rippleAlpha = follow(this.rippleAlpha, state === "speaking" ? 1 : 0, dt, 200, 400);
    this.ripple = (this.ripple + (dt / 1000) * (0.5 + this.level * 0.9)) % 1;

    this.alarm = follow(this.alarm, state === "error" ? 1 : 0, dt, 90, 500);
    if (state === "error") {
      const r = noise(Math.floor(this.time * 22));
      this.flicker = r > 0.55 ? 0.35 : r < -0.8 ? 0.6 : 1;
    } else this.flicker = 1;

    return {
      state,
      intro,
      scale,
      energy: this.energy * this.flicker,
      level: this.level,
      bands: this.bands,
      rot: this.rot,
      sweep: this.sweep,
      sweepAlpha: this.sweepAlpha,
      ripple: this.ripple,
      rippleAlpha: this.rippleAlpha,
      alarm: this.alarm,
      t: this.time,
    };
  }

  /** A settled still frame for `state` (reduced motion / Battery Saver / previews). */
  still(state: JarvisState, level: number): ReactorFrame {
    this.state = state;
    this.introStart = -Infinity;
    this.level = state === "idle" ? 0 : state === "done" ? 0.06 : state === "thinking" ? 0.22 : Math.max(0.2, level);
    syntheticBands(this.level, 1.3, this.bands);
    this.energy = state === "idle" ? 0 : state === "done" ? 0.6 : 1;
    this.scale = state === "done" ? 0.9 : state === "idle" ? 0.6 : 1;
    this.alarm = state === "error" ? 1 : 0;
    this.sweepAlpha = state === "thinking" ? 1 : 0;
    this.rippleAlpha = state === "speaking" ? 1 : 0;
    return {
      state,
      intro: 1,
      scale: this.scale,
      energy: this.energy,
      level: this.level,
      bands: this.bands,
      rot: [0.3, 1.1, 0.6, 2.2],
      sweep: 0.8,
      sweepAlpha: this.sweepAlpha,
      ripple: 0.4,
      rippleAlpha: this.rippleAlpha,
      alarm: this.alarm,
      t: 1.3,
    };
  }
}

// ---------------------------------------------------------------------------
// Text: words (fade-in) and the typed reply.
// ---------------------------------------------------------------------------

/** Words with their trailing whitespace (stable keys for the fade-in). */
export function splitWords(text: string): string[] {
  return text.match(/\S+\s*/g) ?? [];
}

/** Characters per second the reply types at when no speech progress is known. */
export const TYPE_CPS = 55;

export interface TypedInput {
  length: number;
  /** Ms since the reply appeared. */
  elapsedMs: number;
  /** Spoken share 0..1 from the TTS (null = unknown). */
  progress: number | null;
  /** The TTS is speaking this reply. */
  speaking: boolean;
  /** Typing animation allowed (false = all at once). */
  animate: boolean;
}

/**
 * How many characters of the reply to show: follows the speech (a word
 * ahead) while it is spoken, else types at {@link TYPE_CPS}.
 */
export function typedChars(i: TypedInput): number {
  if (!i.animate || i.length === 0) return i.length;
  const timed = Math.floor((i.elapsedMs / 1000) * TYPE_CPS);
  if (i.speaking && i.progress !== null) {
    const spoken = Math.ceil(i.progress * i.length) + 8;
    return Math.min(i.length, Math.max(spoken, Math.floor(timed * 0.35)));
  }
  return Math.min(i.length, timed);
}

/** Snaps a character count to the end of a word (no half words while typing). */
export function snapToWord(text: string, n: number): number {
  if (n >= text.length) return text.length;
  if (n <= 0) return 0;
  const next = text.indexOf(" ", n);
  return next === -1 ? text.length : next;
}
