/**
 * Boot sound (ADR-0053): a soft synthesized whoosh rising into the core
 * ignition, a low swell and a two-note chime, rendered with WebAudio and
 * scheduled ahead on the audio thread. Whether it plays is decided by
 * `bootSoundAllowed` from the mirrored sound settings: the setting
 * (always / after install & updates / off), the sound master switch and
 * volume, Do Not Disturb, quiet hours and Battery Saver.
 */
import { inQuietHours } from "../voice/notify/policy";
import { masterGain } from "../sound/policy";
import type { BootEnv, BootWhen } from "./prefs";

export function bootSoundAllowed(pref: BootWhen, firstLaunch: boolean, env: BootEnv, now: Date = new Date()): boolean {
  if (pref === "off" || (pref === "firstLaunch" && !firstLaunch)) return false;
  if (!env.soundEnabled || env.volumePct <= 0) return false;
  if (env.doNotDisturb && env.muteInDnd) return false;
  if (env.quietHours && inQuietHours(env.quietHours, now)) return false;
  if (env.performanceMode === "BatterySaver") return false;
  return true;
}

/**
 * Plays the boot sound; `igniteS` is when the core ignites (s from now).
 * Returns false when audio is unavailable or blocked (autoplay policy).
 */
export async function playBootSound(volumePct: number, igniteS: number, short: boolean): Promise<boolean> {
  const Ctor: typeof AudioContext | undefined = typeof window === "undefined" ? undefined : window.AudioContext;
  if (!Ctor) return false;
  let c: AudioContext;
  try {
    c = new Ctor({ latencyHint: "interactive" });
  } catch {
    return false;
  }
  try {
    if (c.state === "suspended") await Promise.race([c.resume(), new Promise((r) => setTimeout(r, 250))]);
  } catch {
    /* blocked */
  }
  if (c.state !== "running") {
    void c.close().catch(() => undefined);
    return false;
  }
  const vol = masterGain(volumePct) * 0.55;
  const t0 = c.currentTime + 0.03;
  const ign = t0 + Math.max(0.2, igniteS);
  const master = c.createGain();
  master.gain.value = vol;
  const lp = c.createBiquadFilter();
  lp.type = "lowpass";
  lp.frequency.value = 7000;
  lp.connect(master);
  master.connect(c.destination);
  // Simple feedback echo for air.
  const delay = c.createDelay(1);
  delay.delayTime.value = 0.19;
  const fb = c.createGain();
  fb.gain.value = 0.32;
  const wet = c.createGain();
  wet.gain.value = 0.35;
  delay.connect(fb);
  fb.connect(delay);
  delay.connect(wet);
  wet.connect(lp);

  const env = (g: GainNode, start: number, attack: number, peak: number, end: number) => {
    g.gain.setValueAtTime(0.0001, start);
    g.gain.exponentialRampToValueAtTime(Math.max(0.0002, peak), start + attack);
    g.gain.exponentialRampToValueAtTime(0.0001, end);
  };

  // Whoosh: band-passed noise sweeping up into the ignition.
  const whooshStart = short ? t0 : t0 + 0.1;
  const whooshDur = Math.max(0.3, ign - whooshStart + 0.25);
  const len = Math.floor(c.sampleRate * (whooshDur + 0.1));
  const buf = c.createBuffer(1, len, c.sampleRate);
  const data = buf.getChannelData(0);
  let seed = 99;
  for (let i = 0; i < len; i++) {
    seed = (seed * 1664525 + 1013904223) >>> 0;
    data[i] = (seed / 0xffffffff) * 2 - 1;
  }
  const noise = c.createBufferSource();
  noise.buffer = buf;
  const bp = c.createBiquadFilter();
  bp.type = "bandpass";
  bp.Q.value = 1.4;
  bp.frequency.setValueAtTime(260, whooshStart);
  bp.frequency.exponentialRampToValueAtTime(3200, ign);
  bp.frequency.exponentialRampToValueAtTime(900, ign + 0.25);
  const ng = c.createGain();
  ng.gain.setValueAtTime(0.0001, whooshStart);
  ng.gain.exponentialRampToValueAtTime(short ? 0.12 : 0.16, ign - 0.05);
  ng.gain.exponentialRampToValueAtTime(0.0001, ign + 0.3);
  noise.connect(bp);
  bp.connect(ng);
  ng.connect(lp);
  noise.start(whooshStart);
  noise.stop(ign + 0.35);

  // Low swell (sub) under the whoosh.
  const sub = c.createOscillator();
  sub.type = "sine";
  sub.frequency.setValueAtTime(55, whooshStart);
  sub.frequency.exponentialRampToValueAtTime(82.4, ign);
  const sg = c.createGain();
  env(sg, whooshStart, Math.max(0.1, ign - whooshStart), short ? 0.18 : 0.25, ign + 0.9);
  sub.connect(sg);
  sg.connect(lp);
  sub.start(whooshStart);
  sub.stop(ign + 1);

  // Chime at the ignition: E5 + B5 (+ soft octave), into the echo.
  const notes = short ? [659.25, 987.77] : [659.25, 987.77, 1318.5];
  notes.forEach((f, i) => {
    const o = c.createOscillator();
    o.type = i === 2 ? "triangle" : "sine";
    o.frequency.value = f;
    const g = c.createGain();
    const st = ign + i * 0.07;
    env(g, st, 0.012, (i === 2 ? 0.05 : 0.12) * (short ? 0.8 : 1), st + (short ? 1.1 : 1.8));
    o.connect(g);
    g.connect(lp);
    g.connect(delay);
    o.start(st);
    o.stop(st + 2);
  });

  window.setTimeout(() => void c.close().catch(() => undefined), (ign - c.currentTime + 3.5) * 1000);
  return true;
}
