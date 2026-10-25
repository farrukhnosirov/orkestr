/**
 * Jarvis signal frames between windows (pure): the main window quantizes a
 * sample into plain numbers for the orb channel; the orb window turns the
 * latest frame back into a {@link JarvisSignal}.
 */
import type { JarvisSignal, SignalSample } from "./model";

/** Orb channel frame: bands quantized to 0..100 (plain numbers). */
export interface SignalFrame {
  level: number;
  bands: number[];
  progress: number | null;
}

/** Quantizes a sample for the channel. */
export function toFrame(sample: SignalSample, bands: Float32Array): SignalFrame {
  return {
    level: Math.round(sample.level * 100) / 100,
    bands: Array.from(bands, (v) => Math.round(v * 100)),
    progress: sample.progress === null ? null : Math.round(sample.progress * 1000) / 1000,
  };
}

/** Whether a message payload is a well-formed frame. */
export function isSignalFrame(v: unknown): v is SignalFrame {
  if (!v || typeof v !== "object") return false;
  const f = v as Partial<SignalFrame>;
  return typeof f.level === "number" && Array.isArray(f.bands) && f.bands.length <= 128 && (f.progress === null || typeof f.progress === "number");
}

/** Orb window: a signal fed by channel frames (stale after 400 ms → synthesized). */
export function channelSignal(): JarvisSignal & { push: (f: SignalFrame) => void } {
  let last: SignalFrame | null = null;
  let at = 0;
  return {
    push(f) {
      last = f;
      at = performance.now();
    },
    sample(out, now) {
      if (!last || now - at > 400) return null;
      for (let i = 0; i < out.length; i++) out[i] = (last.bands[i] ?? 0) / 100;
      return { level: last.level, progress: last.progress };
    },
  };
}
