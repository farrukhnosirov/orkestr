/**
 * Procedural robot animation: a pure function from (state, time, intensity)
 * to a pose. Both renderers (2D SVG and 3D three.js) draw the same pose, so
 * every state looks the same in both and the motion is unit-testable.
 *
 * Units: px in the 200×200 robot viewBox, degrees, 0..1 factors.
 */
import type { RobotState } from "./states";

export type EyeShape = "round" | "happy" | "x" | "closed" | "focus" | "up";
export type MouthShape = "smile" | "flat" | "o" | "grin" | "frown" | "wave";
/** The prop a state shows (drawn next to / in the hands of the robot). */
export type RobotProp = "none" | "waves" | "thought" | "keyboard" | "tube" | "magnifier" | "hourglass" | "sparks" | "confetti" | "sleep";

export interface RobotPose {
  /** Whole-body vertical offset (negative = up). */
  bodyY: number;
  /** Body sway (deg). */
  tilt: number;
  /** Horizontal shake (error). */
  shakeX: number;
  /** Vertical squash/stretch (1 = none). */
  squash: number;
  headTilt: number;
  headY: number;
  /** Eye openness 0..1 (blinks). */
  eyeOpen: number;
  eyes: EyeShape;
  /** Gaze -1..1. */
  lookX: number;
  lookY: number;
  mouth: MouthShape;
  /** Arm angles in degrees from hanging down (positive = outward/up). */
  armL: number;
  armR: number;
  /** Antenna light 0..1. */
  antenna: number;
  /** Face screen glow 0..1. */
  glow: number;
  prop: RobotProp;
  /** Cyclic phase 0..1 for prop animation. */
  phase: number;
}

const TAU = Math.PI * 2;
const wave = (t: number, periodMs: number, offset = 0) => Math.sin((t / periodMs) * TAU + offset);
const cycle = (t: number, periodMs: number) => (((t % periodMs) + periodMs) % periodMs) / periodMs;

/** Eye openness with a 140 ms blink every `everyMs`. */
export function blink(t: number, everyMs = 3600): number {
  const p = ((t % everyMs) + everyMs) % everyMs;
  if (p > 140) return 1;
  return Math.abs(p - 70) / 70;
}

/** A representative frame for static rendering (Battery Saver / reduced motion). */
export const STATIC_FRAME_MS: Readonly<Record<RobotState, number>> = {
  Idle: 1000,
  Listening: 400,
  Thinking: 600,
  Coding: 250,
  Testing: 700,
  Reviewing: 900,
  Waiting: 500,
  Error: 300,
  Completed: 450,
};

/**
 * The pose of a robot in `state` at time `t` (ms). `intensity` (0..1) scales
 * every amplitude; 0 is a still robot that still shows its state through
 * eyes, mouth, colors and props. `asleep` = stopped agent.
 */
export function poseAt(state: RobotState, t: number, intensity = 1, asleep = false): RobotPose {
  const k = Math.min(1, Math.max(0, intensity));
  const breathe = wave(t, 3200);
  const base: RobotPose = {
    bodyY: breathe * 2 * k,
    tilt: 0,
    shakeX: 0,
    squash: 1 + breathe * 0.012 * k,
    headTilt: wave(t, 5200) * 2 * k,
    headY: 0,
    eyeOpen: blink(t),
    eyes: "round",
    lookX: wave(t, 7000) * 0.3 * k,
    lookY: 0,
    mouth: "smile",
    armL: 6,
    armR: 6,
    antenna: 0.45 + 0.25 * (wave(t, 2400) + 1) / 2,
    glow: 0.5,
    prop: "none",
    phase: cycle(t, 2000),
  };
  if (asleep) {
    return {
      ...base,
      bodyY: 3 + breathe * 1.2 * k,
      headTilt: 8,
      headY: 3,
      eyes: "closed",
      eyeOpen: 0,
      lookX: 0,
      mouth: "flat",
      armL: 2,
      armR: 2,
      antenna: 0.1,
      glow: 0.15,
      prop: "sleep",
      phase: cycle(t, 3000),
    };
  }
  switch (state) {
    case "Idle":
      return base;
    case "Listening":
      return {
        ...base,
        headTilt: 10 + wave(t, 1800) * 3 * k,
        eyes: "round",
        lookX: 0.5,
        mouth: "o",
        armR: 40 + wave(t, 1800) * 6 * k,
        antenna: 0.6 + 0.4 * Math.abs(wave(t, 900)),
        glow: 0.7,
        prop: "waves",
        phase: cycle(t, 1200),
      };
    case "Thinking":
      return {
        ...base,
        headTilt: -8 + wave(t, 2600) * 4 * k,
        eyes: "up",
        lookX: 0.6 + wave(t, 2600) * 0.3 * k,
        lookY: -0.7,
        mouth: "flat",
        armR: 120 + wave(t, 1300) * 8 * k,
        antenna: 0.5 + 0.5 * Math.abs(wave(t, 700)),
        glow: 0.65,
        prop: "thought",
        phase: cycle(t, 1800),
      };
    case "Coding": {
      const tap = wave(t, 180);
      return {
        ...base,
        bodyY: base.bodyY * 0.5,
        headTilt: 4,
        eyes: "focus",
        lookX: wave(t, 900) * 0.5 * k,
        lookY: 0.6,
        mouth: "flat",
        // Negative = hands in front of the body, typing on the holo keyboard.
        armL: -32 + tap * 8 * k,
        armR: -32 - tap * 8 * k,
        antenna: 0.65 + 0.35 * Math.abs(wave(t, 400)),
        glow: 0.9,
        prop: "keyboard",
        phase: cycle(t, 900),
      };
    }
    case "Testing":
      return {
        ...base,
        tilt: wave(t, 1600) * 3 * k,
        eyes: "round",
        lookX: 0.7,
        lookY: -0.2,
        mouth: "o",
        armR: 95 + wave(t, 800) * 12 * k,
        antenna: 0.5 + 0.5 * Math.abs(wave(t, 500)),
        glow: 0.75,
        prop: "tube",
        phase: cycle(t, 1400),
      };
    case "Reviewing": {
      const sweep = wave(t, 2400);
      return {
        ...base,
        headTilt: sweep * 6 * k,
        eyes: "focus",
        lookX: sweep * 0.9,
        lookY: 0.1,
        mouth: "flat",
        armR: 80 + sweep * 18 * k,
        antenna: 0.55,
        glow: 0.7,
        prop: "magnifier",
        phase: (sweep + 1) / 2,
      };
    }
    case "Waiting":
      return {
        ...base,
        bodyY: base.bodyY * 0.6,
        headTilt: 6 + wave(t, 4000) * 4 * k,
        eyes: "round",
        lookX: wave(t, 4000) * 0.8 * k,
        lookY: -0.2,
        mouth: "flat",
        armL: 18 + Math.max(0, wave(t, 1600)) * 6 * k,
        armR: 18,
        antenna: 0.3 + 0.7 * Math.max(0, wave(t, 1600)),
        glow: 0.45,
        prop: "hourglass",
        phase: cycle(t, 3000),
      };
    case "Error": {
      const flick = Math.sin(t / 37) * Math.sin(t / 53);
      return {
        ...base,
        shakeX: flick * 2.5 * k,
        tilt: -4,
        headTilt: -10,
        eyes: "x",
        eyeOpen: 1,
        lookX: 0,
        mouth: "frown",
        armL: 4,
        armR: 4,
        antenna: Math.abs(flick) > 0.4 ? 1 : 0.15,
        glow: 0.5 + flick * 0.3,
        prop: "sparks",
        phase: cycle(t, 600),
      };
    }
    case "Completed": {
      const hop = Math.max(0, wave(t, 900));
      return {
        ...base,
        bodyY: -hop * 10 * k,
        squash: 1 - (1 - hop) * 0.04 * k,
        headTilt: wave(t, 900) * 6 * k,
        eyes: "happy",
        eyeOpen: 1,
        lookX: 0,
        mouth: "grin",
        armL: 150 + hop * 20 * k,
        armR: 150 + hop * 20 * k,
        antenna: 1,
        glow: 1,
        prop: "confetti",
        phase: cycle(t, 1800),
      };
    }
  }
}

/** Deterministic pseudo-random in [0,1) for particle layouts. */
export function hash01(n: number): number {
  const x = Math.sin(n * 127.1 + 311.7) * 43758.5453;
  return x - Math.floor(x);
}
