/**
 * Speech-to-text engines (§33). Every engine here processes audio off the
 * device, so the core only lets the user select one after an explicit
 * consent (`voice.setConsent`) and the pipeline starts a session only after
 * push-to-talk or the local wake word — never continuously.
 *
 * - `WebSpeech`: the webview's `SpeechRecognition` (WebView2/Chromium send
 *   audio to the browser vendor's service). Detected at runtime.
 * - Core engines (ADR-0046: whisper.cpp, Vosk, sherpa-onnx, Windows speech
 *   — offline; OpenAI, Deepgram, Azure, Google, Yandex — cloud, consent):
 *   16 kHz PCM from the voice worklet → WAV → `voice.transcribe`; the core
 *   picks the per-language engine and walks the fallback chain.
 * - `Disabled`: no recognition; the HUD takes typed commands.
 */
import type { SttEngine } from "@orkestr/protocol";

import { maybeClient } from "../../../core/client";
import type { MicCapture } from "../audio/mic";
import { concatPcm, encodeWav, peak, toBase64 } from "../audio/wav";
import { SAMPLE_RATE } from "../wake/mfcc";

/** Max command length (ADR-0017: 15 s). */
export const MAX_COMMAND_MS = 15_000;
/** End of command after this much silence (ADR-0017: 800 ms). */
export const END_SILENCE_MS = 900;

export interface SttSession {
  /** Interim text (WebSpeech only). */
  onInterim: (cb: (text: string) => void) => void;
  /** Called when the engine decides the utterance is over (silence / limit). */
  onAutoEnd: (cb: () => void) => void;
  /** Stops listening and returns the final transcript. */
  finish: () => Promise<string>;
  /** Aborts without a transcript. */
  cancel: () => void;
}

export class SttError extends Error {
  constructor(
    readonly code: "unsupported" | "denied" | "network" | "noSpeech" | "failed",
    message: string,
  ) {
    super(message);
  }
}

// --- Web Speech --------------------------------------------------------------

interface SpeechRecognitionAlternativeLike {
  transcript: string;
}
interface SpeechRecognitionResultLike {
  readonly isFinal: boolean;
  readonly length: number;
  [index: number]: SpeechRecognitionAlternativeLike | undefined;
}
interface SpeechRecognitionEventLike {
  readonly resultIndex: number;
  readonly results: { readonly length: number; [index: number]: SpeechRecognitionResultLike | undefined };
}
interface SpeechRecognitionLike {
  lang: string;
  continuous: boolean;
  interimResults: boolean;
  maxAlternatives: number;
  onresult: ((e: SpeechRecognitionEventLike) => void) | null;
  onerror: ((e: { error: string }) => void) | null;
  onend: (() => void) | null;
  start: () => void;
  stop: () => void;
  abort: () => void;
}
type SpeechRecognitionCtor = new () => SpeechRecognitionLike;

function webSpeechCtor(): SpeechRecognitionCtor | null {
  if (typeof window === "undefined") return null;
  const w = window as unknown as { SpeechRecognition?: SpeechRecognitionCtor; webkitSpeechRecognition?: SpeechRecognitionCtor };
  return w.SpeechRecognition ?? w.webkitSpeechRecognition ?? null;
}

/** Whether the webview exposes the Web Speech recognition API. */
export function webSpeechAvailable(): boolean {
  return webSpeechCtor() !== null;
}

function webSpeechSession(lang: string, continuous: boolean): SttSession {
  const Ctor = webSpeechCtor();
  if (!Ctor) throw new SttError("unsupported", "Web Speech recognition is not available");
  const rec = new Ctor();
  rec.lang = lang;
  rec.continuous = continuous;
  rec.interimResults = true;
  rec.maxAlternatives = 1;
  let finalText = "";
  let interim = "";
  let interimCb: (t: string) => void = () => {};
  let autoEndCb: () => void = () => {};
  let error: SttError | null = null;
  let ended = false;
  let endWaiters: Array<() => void> = [];
  rec.onresult = (e) => {
    interim = "";
    for (let i = e.resultIndex; i < e.results.length; i++) {
      const r = e.results[i];
      const text = r?.[0]?.transcript ?? "";
      if (r?.isFinal) finalText = `${finalText} ${text}`.trim();
      else interim += text;
    }
    interimCb(`${finalText} ${interim}`.trim());
  };
  rec.onerror = (e) => {
    const code = e.error === "not-allowed" || e.error === "service-not-allowed" ? "denied" : e.error === "network" ? "network" : e.error === "no-speech" ? "noSpeech" : "failed";
    if (e.error !== "aborted") error = new SttError(code, e.error);
  };
  rec.onend = () => {
    ended = true;
    const w = endWaiters;
    endWaiters = [];
    w.forEach((f) => f());
    autoEndCb();
  };
  rec.start();
  const limit = window.setTimeout(() => rec.stop(), MAX_COMMAND_MS);
  return {
    onInterim: (cb) => {
      interimCb = cb;
    },
    onAutoEnd: (cb) => {
      autoEndCb = cb;
    },
    finish: async () => {
      window.clearTimeout(limit);
      if (!ended) {
        await new Promise<void>((resolve) => {
          endWaiters.push(resolve);
          rec.stop();
          window.setTimeout(resolve, 4000);
        });
      }
      const text = `${finalText} ${interim}`.trim();
      if (!text && error) throw error;
      return text;
    },
    cancel: () => {
      window.clearTimeout(limit);
      autoEndCb = () => {};
      rec.abort();
    },
  };
}

// --- Whisper (PCM via the worklet) ------------------------------------------

function whisperSession(mic: MicCapture, lang: string, autoEnd: boolean): SttSession {
  const chunks: Float32Array[] = [];
  let autoEndCb: () => void = () => {};
  let spoke = false;
  let silentSince = 0;
  const started = Date.now();
  const off = mic.subscribe((m) => {
    if (m.type === "pcm") chunks.push(m.data);
    if (m.type === "level" && autoEnd) {
      if (m.speaking) {
        spoke = true;
        silentSince = 0;
      } else if (spoke) {
        silentSince ||= Date.now();
        if (Date.now() - silentSince > END_SILENCE_MS) autoEndCb();
      } else if (Date.now() - started > 6000) {
        autoEndCb(); // nothing said after the wake word
      }
    }
  });
  mic.setRecording(true);
  const limit = window.setTimeout(() => autoEndCb(), MAX_COMMAND_MS);
  const stop = () => {
    window.clearTimeout(limit);
    mic.setRecording(false);
    off();
  };
  return {
    onInterim: () => {},
    onAutoEnd: (cb) => {
      autoEndCb = cb;
    },
    finish: async () => {
      stop();
      const pcm = concatPcm(chunks, SAMPLE_RATE * (MAX_COMMAND_MS / 1000));
      chunks.length = 0; // audio buffers are dropped right after use (ADR-0017)
      if (pcm.length < SAMPLE_RATE * 0.3 || peak(pcm) < 0.01) throw new SttError("noSpeech", "no speech captured");
      const client = maybeClient();
      if (!client) throw new SttError("network", "not connected");
      const audioBase64 = toBase64(encodeWav(pcm, SAMPLE_RATE));
      const r = await client.call("voice.transcribe", { audioBase64, mimeType: "audio/wav", language: lang.split("-")[0] ?? lang }, { timeoutMs: 60_000 });
      return r.text.trim();
    },
    cancel: () => {
      autoEndCb = () => {};
      stop();
      chunks.length = 0;
    },
  };
}

export interface StartSttOptions {
  engine: SttEngine;
  lang: string;
  /** Needed for Whisper (PCM source). */
  mic: MicCapture | null;
  /** End on silence (wake word) instead of on key release (push-to-talk). */
  autoEnd: boolean;
}

/** Starts a recognition session, or throws `SttError`. */
export function startStt(o: StartSttOptions): SttSession {
  switch (o.engine) {
    case "WebSpeech":
      return webSpeechSession(o.lang, !o.autoEnd);
    case "Disabled":
      throw new SttError("unsupported", "speech recognition is disabled");
    default:
      if (!o.mic) throw new SttError("unsupported", "microphone not open");
      return whisperSession(o.mic, o.lang, o.autoEnd);
  }
}
