/**
 * TTS output tap for the Jarvis visual: the spoken reply's amplitude /
 * spectrum and how much of it was spoken. Core engines play through an
 * `<audio>` element that is routed through a small output `AudioContext`
 * with an `AnalyserNode` (playback only — no microphone); the system voice
 * (`speechSynthesis`) exposes no audio, so its word boundaries drive a
 * pulse instead. Routing is skipped when the context cannot run (autoplay
 * policy) so a reply is never muted.
 */

let ctx: AudioContext | null = null;
let analyser: AnalyserNode | null = null;
let tapped: WeakSet<HTMLMediaElement> = new WeakSet();
let progressOf: (() => number | null) | null = null;
let boundaryAt = 0;
let active = false;
/** The current utterance plays through the analyser. */
let routed = false;

async function outputContext(): Promise<AudioContext | null> {
  try {
    if (typeof AudioContext !== "function") return null;
    if (!ctx) {
      ctx = new AudioContext({ latencyHint: "playback" });
      analyser = ctx.createAnalyser();
      analyser.fftSize = 512;
      analyser.smoothingTimeConstant = 0.55;
      analyser.connect(ctx.destination);
    }
    if (ctx.state !== "running") {
      await Promise.race([ctx.resume().catch(() => undefined), new Promise((r) => setTimeout(r, 150))]);
    }
    return ctx.state === "running" ? ctx : null;
  } catch {
    return null;
  }
}

/** Before `audio.play()`: routes the element through the analyser when possible. */
export async function tapAudioElement(audio: HTMLAudioElement): Promise<void> {
  active = true;
  routed = false;
  progressOf = () => (Number.isFinite(audio.duration) && audio.duration > 0 ? Math.min(1, audio.currentTime / audio.duration) : null);
  if (tapped.has(audio)) {
    routed = true;
    return;
  }
  const c = await outputContext();
  if (!c || !analyser) return;
  try {
    c.createMediaElementSource(audio).connect(analyser);
    tapped.add(audio);
    routed = true;
  } catch {
    /* already routed / unsupported: the visual pulses instead */
  }
}

/** System voice started: progress from word boundaries. */
export function systemSpeechStarted(length: number): void {
  active = true;
  routed = false;
  let spoken = 0;
  boundaryAt = performance.now();
  progressOf = () => (length > 0 ? Math.min(1, spoken / length) : null);
  systemBoundary = (charIndex: number) => {
    spoken = charIndex;
    boundaryAt = performance.now();
  };
}

let systemBoundary: ((charIndex: number) => void) | null = null;

/** `SpeechSynthesisUtterance.onboundary`. */
export function systemSpeechBoundary(charIndex: number): void {
  systemBoundary?.(charIndex);
}

export function speechEnded(): void {
  active = false;
  routed = false;
  progressOf = null;
  systemBoundary = null;
}

/** Live output: analyser (core engines, while routed), progress, and a word pulse 0..1. */
export function ttsOutput(): { analyser: AnalyserNode | null; sampleRate: number; progress: number | null; pulse: number; active: boolean } {
  const pulse = Math.exp(-(performance.now() - boundaryAt) / 220);
  return { analyser: active && routed && ctx?.state === "running" ? analyser : null, sampleRate: ctx?.sampleRate ?? 48_000, progress: progressOf?.() ?? null, pulse, active };
}

/** Test seam. */
export function resetOutputTap(): void {
  tapped = new WeakSet();
  progressOf = null;
  systemBoundary = null;
  active = false;
}
