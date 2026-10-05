/**
 * WebAudio renderer of the procedural sound recipes (§33 Sound Engine).
 *
 * - One lazily created `AudioContext` (created on first sound, after a user
 *   gesture in practice), a master gain and a shared low-pass; reverb is a
 *   procedurally generated impulse response, only in full quality.
 * - Every node is scheduled ahead on the audio thread; nothing runs on the
 *   UI thread after scheduling. Nodes disconnect themselves when done.
 * - What may play is decided by `policy.ts` from a context getter that the
 *   app registers (`configureSound`), so this module has no store imports.
 */
import type { SoundPreset, ThemeSounds } from "@orkestr/protocol";

import { CRITICAL_CUES, masterGain, shouldPlaySound, soundQuality, type SoundContext } from "./policy";
import { recipeFor, resolvePreset, THEME_SOUND_PRESETS, themeCueOverride, type ResolvedPreset, type SoundCue, type SoundQuality, type SoundRecipe } from "./recipes";

export interface SoundEnvironment extends SoundContext {
  themeId: string;
  /** The active theme's sound mapping (§29); absent = engine defaults. */
  themeSounds?: ThemeSounds | null;
}

let getEnvironment: (() => SoundEnvironment | null) | null = null;
let ctx: AudioContext | null = null;
let reverbBuffer: AudioBuffer | null = null;
let lastPlayed = new Map<SoundCue, number>();

/** Registers the source of the sound context (config, DND, performance mode, theme). */
export function configureSound(getter: () => SoundEnvironment | null): void {
  getEnvironment = getter;
}

function audioContext(): AudioContext | null {
  if (ctx) return ctx;
  const Ctor: typeof AudioContext | undefined =
    typeof window === "undefined" ? undefined : (window.AudioContext ?? (window as unknown as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext);
  if (!Ctor) return null;
  try {
    ctx = new Ctor({ latencyHint: "interactive" });
  } catch {
    return null;
  }
  return ctx;
}

function impulse(c: AudioContext): AudioBuffer {
  if (reverbBuffer && reverbBuffer.sampleRate === c.sampleRate) return reverbBuffer;
  const len = Math.floor(c.sampleRate * 1.6);
  const buf = c.createBuffer(2, len, c.sampleRate);
  for (let ch = 0; ch < 2; ch++) {
    const data = buf.getChannelData(ch);
    let seed = 1234567 + ch * 7919;
    for (let i = 0; i < len; i++) {
      seed = (seed * 1103515245 + 12345) >>> 0;
      const n = (seed / 0xffffffff) * 2 - 1;
      data[i] = n * (1 - i / len) ** 3.2;
    }
  }
  reverbBuffer = buf;
  return buf;
}

function noiseBuffer(c: AudioContext, seconds: number): AudioBuffer {
  const len = Math.max(1, Math.floor(c.sampleRate * seconds));
  const buf = c.createBuffer(1, len, c.sampleRate);
  const data = buf.getChannelData(0);
  let seed = 42;
  for (let i = 0; i < len; i++) {
    seed = (seed * 1664525 + 1013904223) >>> 0;
    data[i] = (seed / 0xffffffff) * 2 - 1;
  }
  return buf;
}

/** Renders a recipe at `volume` (0..1 master gain). */
export function renderRecipe(c: AudioContext, recipe: SoundRecipe, volume: number): void {
  const t0 = c.currentTime + 0.01;
  const master = c.createGain();
  master.gain.value = volume;
  const lowpass = c.createBiquadFilter();
  lowpass.type = "lowpass";
  lowpass.frequency.value = recipe.lowpassHz;
  lowpass.connect(master);
  master.connect(c.destination);
  let send: GainNode | null = null;
  if (recipe.reverb > 0) {
    const conv = c.createConvolver();
    conv.buffer = impulse(c);
    send = c.createGain();
    send.gain.value = recipe.reverb;
    send.connect(conv);
    conv.connect(master);
  }
  let end = t0;
  for (const tone of recipe.tones) {
    const osc = c.createOscillator();
    osc.type = tone.wave;
    const start = t0 + tone.at;
    const stop = start + tone.dur;
    osc.frequency.setValueAtTime(tone.freq, start);
    if (tone.freqEnd) osc.frequency.exponentialRampToValueAtTime(Math.max(20, tone.freqEnd), stop);
    if (tone.detune) osc.detune.value = tone.detune;
    const env = c.createGain();
    env.gain.setValueAtTime(0.0001, start);
    env.gain.exponentialRampToValueAtTime(Math.max(0.0002, tone.gain), start + tone.attack);
    env.gain.exponentialRampToValueAtTime(0.0001, stop);
    osc.connect(env);
    env.connect(lowpass);
    if (send) env.connect(send);
    osc.start(start);
    osc.stop(stop + 0.02);
    osc.onended = () => {
      osc.disconnect();
      env.disconnect();
    };
    end = Math.max(end, stop);
  }
  if (recipe.noise) {
    const n = recipe.noise;
    const src = c.createBufferSource();
    src.buffer = noiseBuffer(c, n.dur);
    const bp = c.createBiquadFilter();
    bp.type = "bandpass";
    bp.frequency.setValueAtTime(n.filterHz * 0.4, t0 + n.at);
    bp.frequency.exponentialRampToValueAtTime(n.filterHz * 2, t0 + n.at + n.dur);
    const env = c.createGain();
    env.gain.setValueAtTime(0.0001, t0 + n.at);
    env.gain.exponentialRampToValueAtTime(Math.max(0.0002, n.gain), t0 + n.at + n.dur * 0.5);
    env.gain.exponentialRampToValueAtTime(0.0001, t0 + n.at + n.dur);
    src.connect(bp);
    bp.connect(env);
    env.connect(lowpass);
    src.start(t0 + n.at);
    src.onended = () => {
      src.disconnect();
      bp.disconnect();
      env.disconnect();
    };
    end = Math.max(end, t0 + n.at + n.dur);
  }
  // Tear down the shared chain after the tail (incl. reverb).
  const tailMs = (end - c.currentTime + (send ? 1.8 : 0.1)) * 1000;
  window.setTimeout(() => {
    lowpass.disconnect();
    master.disconnect();
    send?.disconnect();
  }, tailMs);
}

export interface PlayOptions {
  /** Settings "test" button: ignores DND / battery saver. */
  test?: boolean;
  /** Override preset (settings preview). */
  preset?: SoundPreset;
  /** Override quality. */
  quality?: SoundQuality;
}

/** Minimum gap between two identical cues (event bursts must not machine-gun). */
export const CUE_COOLDOWN_MS = 350;

/** Plays a cue if the policy allows it. Returns whether it was scheduled. */
export function playSound(cue: SoundCue, opts: PlayOptions = {}): boolean {
  const env = getEnvironment?.() ?? null;
  if (!env) return false;
  if (!shouldPlaySound(cue, { ...env, test: opts.test ?? false })) return false;
  // The theme may silence or re-voice a cue (critical alerts are never silenced).
  const theme = opts.test || opts.preset ? null : themeCueOverride(cue, env.themeSounds);
  if (theme?.mute && !CRITICAL_CUES.has(cue)) return false;
  const now = Date.now();
  if (!opts.test && now - (lastPlayed.get(cue) ?? 0) < CUE_COOLDOWN_MS) return false;
  const c = audioContext();
  if (!c) return false;
  lastPlayed.set(cue, now);
  if (c.state === "suspended") void c.resume().catch(() => {});
  const preset: ResolvedPreset = theme?.preset ?? resolvePreset(opts.preset ?? env.config.preset, env.themeId);
  const gain = masterGain(env.config.volumePct) * (theme && !theme.mute ? theme.volume : 1);
  try {
    renderRecipe(c, recipeFor(cue, preset, opts.quality ?? soundQuality(env.performanceMode)), gain);
    return true;
  } catch (e) {
    console.warn("sound engine", e);
    return false;
  }
}

/** Test helper: forget cooldowns. */
export function resetSoundCooldowns(): void {
  lastPlayed = new Map();
}

/**
 * Previews a theme preset id (theme editor play buttons): plays `cue` with
 * the engine voice behind `presetId` at `volume` (0..1), ignoring DND.
 */
export function playThemePreset(presetId: string, cue: SoundCue, volume: number): boolean {
  const voice = THEME_SOUND_PRESETS[presetId];
  if (!voice) return false;
  const env = getEnvironment?.() ?? null;
  const c = audioContext();
  if (!c) return false;
  if (c.state === "suspended") void c.resume().catch(() => {});
  const base = env ? masterGain(env.config.volumePct) : 0.5;
  try {
    renderRecipe(c, recipeFor(cue, voice, soundQuality(env?.performanceMode ?? "Balanced")), base * Math.min(1, Math.max(0, volume)));
    return true;
  } catch (err) {
    console.warn("sound preview", err);
    return false;
  }
}
