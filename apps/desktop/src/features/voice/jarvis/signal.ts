/**
 * Live signals for the Jarvis reactor.
 *
 * - {@link liveVoiceSignal} (main window): the open microphone's analyser
 *   while listening (the same capture the command uses — no second mic) and
 *   the TTS output tap while speaking; the system voice pulses on word
 *   boundaries.
 * - {@link startSignalBroadcast} (→ `channelSignal` in `frame.ts`): the orb window has no
 *   audio, so the main window sends it the band energies (plain numbers,
 *   never audio) at ~30 Hz while a voice command runs.
 */
import { currentAnalyser } from "../controller";
import { useVoiceStore } from "../store";
import { ttsOutput } from "../tts/outputTap";
import { type SignalFrame, toFrame } from "./frame";
import { BANDS, bandsFromFrequency, type JarvisSignal, levelOfBands, syntheticBands } from "./model";

let freq: Uint8Array<ArrayBuffer> | null = null;

function analyserBands(a: AnalyserNode, sampleRate: number, out: Float32Array): void {
  if (!freq || freq.length !== a.frequencyBinCount) freq = new Uint8Array(a.frequencyBinCount);
  a.getByteFrequencyData(freq);
  bandsFromFrequency(freq, out, sampleRate);
}

/** Main window: mic while listening, TTS while speaking, else nothing live. */
export const liveVoiceSignal: JarvisSignal = {
  sample(out, now) {
    const s = useVoiceStore.getState();
    if (s.speaking) {
      const tts = ttsOutput();
      if (tts.analyser) {
        analyserBands(tts.analyser, tts.sampleRate, out);
        return { level: Math.min(1, levelOfBands(out) * 1.6), progress: tts.progress };
      }
      // System voice: a syllable-ish envelope pulsing on each word.
      const level = 0.3 + 0.45 * tts.pulse + 0.12 * Math.sin(now / 55) * Math.sin(now / 131);
      syntheticBands(level, now / 1000, out);
      return { level, progress: tts.progress };
    }
    if (s.phase === "listening") {
      const a = currentAnalyser();
      if (a) {
        analyserBands(a, a.context.sampleRate, out);
        return { level: Math.max(s.level, levelOfBands(out)), progress: null };
      }
      syntheticBands(s.level, now / 1000, out);
      return { level: s.level, progress: null };
    }
    return null;
  },
};

/**
 * Main window: posts frames through `post` while a command runs or a reply
 * is spoken (`setInterval`: keeps going while the window is in the
 * background, unlike `requestAnimationFrame`). Returns a disposer.
 */
export function startSignalBroadcast(post: (f: SignalFrame) => void, hz = 30): () => void {
  const bands = new Float32Array(BANDS);
  const timer = window.setInterval(() => {
    const st = useVoiceStore.getState();
    if (st.phase === "idle" && !st.speaking) return;
    const s = liveVoiceSignal.sample(bands, performance.now());
    if (s) post(toFrame(s, bands));
  }, Math.round(1000 / hz));
  return () => window.clearInterval(timer);
}
