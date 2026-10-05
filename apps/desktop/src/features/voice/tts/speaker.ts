/**
 * Text-to-speech (§33): the OS voices through `speechSynthesis` (default,
 * offline) or OpenAI TTS through `voice.synthesize` (opt-in, consent is
 * enforced by the core; only text is sent). One utterance at a time: a new
 * reply interrupts the previous one. Voice per agent maps a template's
 * `VoiceProfile` onto the same options.
 */
import type { TtsEngine, VoiceConfig, VoiceProfile } from "@orkestr/protocol";

import { maybeClient } from "../../../core/client";
import { fromBase64 } from "../audio/wav";

export interface SpeakOptions {
  engine: TtsEngine;
  /** BCP-47 tag, e.g. `ru-RU`. */
  lang: string;
  /** System voice name (`speechSynthesis`), or null for the best match. */
  systemVoice: string | null;
  /** OpenAI voice id. */
  cloudVoice: string;
  style: string | null;
  ratePct: number;
  pitchPct: number;
}

/** UI language → speech locale. */
export function speechLang(configLanguage: string, uiLanguage: string): string {
  const lang = configLanguage === "auto" ? uiLanguage : configLanguage;
  if (lang.includes("-")) return lang;
  if (lang === "ru") return "ru-RU";
  if (lang === "en") return "en-US";
  return lang;
}

export function optionsFromConfig(c: VoiceConfig, uiLanguage: string): SpeakOptions {
  return {
    engine: c.ttsEngine,
    lang: speechLang(c.language, uiLanguage),
    systemVoice: c.systemVoice,
    cloudVoice: c.cloudVoice,
    style: null,
    ratePct: c.ratePct,
    pitchPct: c.pitchPct,
  };
}

/**
 * Options for an agent's voice (template `VoiceProfile`); falls back to the
 * assistant voice for missing fields. Cloud voices are used only when the
 * assistant config has cloud TTS enabled (consent).
 */
export function optionsForProfile(profile: VoiceProfile | null, base: SpeakOptions): SpeakOptions {
  if (!profile) return base;
  const cloud = profile.provider === "openai" && base.engine === "OpenAi";
  return {
    engine: cloud ? "OpenAi" : "System",
    lang: profile.language && profile.language !== "auto" ? speechLang(profile.language, base.lang) : base.lang,
    systemVoice: !cloud && profile.voice ? profile.voice : base.systemVoice,
    cloudVoice: cloud && profile.voice ? profile.voice : base.cloudVoice,
    style: profile.style,
    ratePct: profile.ratePct ?? base.ratePct,
    pitchPct: profile.pitchPct ?? base.pitchPct,
  };
}

export interface VoiceLike {
  name: string;
  lang: string;
  localService: boolean;
  default: boolean;
}

/** Best system voice: exact name, else same locale (local first), else same language. */
export function pickVoice<V extends VoiceLike>(voices: readonly V[], lang: string, preferred: string | null): V | null {
  if (preferred) {
    const exact = voices.find((v) => v.name === preferred);
    if (exact) return exact;
  }
  const norm = (s: string) => s.toLowerCase().replace("_", "-");
  const want = norm(lang);
  const primary = want.split("-")[0] ?? want;
  const rank = (v: V) => {
    const l = norm(v.lang);
    let r = 0;
    if (l === want) r += 4;
    else if (l.split("-")[0] === primary) r += 2;
    else return -1;
    if (v.localService) r += 1;
    if (v.default) r += 0.5;
    return r;
  };
  let best: V | null = null;
  let bestRank = -1;
  for (const v of voices) {
    const r = rank(v);
    if (r > bestRank) {
      best = v;
      bestRank = r;
    }
  }
  return best;
}

export function ttsSupported(): boolean {
  return typeof window !== "undefined" && "speechSynthesis" in window && typeof SpeechSynthesisUtterance !== "undefined";
}

/** System voices; resolves after `voiceschanged` when the list loads lazily. */
export function systemVoices(): Promise<SpeechSynthesisVoice[]> {
  if (!ttsSupported()) return Promise.resolve([]);
  const now = window.speechSynthesis.getVoices();
  if (now.length > 0) return Promise.resolve(now);
  return new Promise((resolve) => {
    const done = () => {
      window.speechSynthesis.removeEventListener("voiceschanged", done);
      resolve(window.speechSynthesis.getVoices());
    };
    window.speechSynthesis.addEventListener("voiceschanged", done);
    window.setTimeout(done, 1500);
  });
}

let currentAudio: HTMLAudioElement | null = null;
let currentUrl: string | null = null;
let speakSeq = 0;

/** Stops whatever is being spoken. */
export function stopSpeaking(): void {
  speakSeq += 1;
  if (ttsSupported()) window.speechSynthesis.cancel();
  if (currentAudio) {
    currentAudio.pause();
    currentAudio = null;
  }
  if (currentUrl) {
    URL.revokeObjectURL(currentUrl);
    currentUrl = null;
  }
}

async function speakSystem(text: string, o: SpeakOptions, seq: number): Promise<void> {
  if (!ttsSupported()) throw new Error("speechSynthesis unavailable");
  const voices = await systemVoices();
  if (seq !== speakSeq) return;
  const u = new SpeechSynthesisUtterance(text);
  const v = pickVoice(voices, o.lang, o.systemVoice);
  if (v) u.voice = v;
  u.lang = v?.lang ?? o.lang;
  u.rate = Math.min(2, Math.max(0.5, o.ratePct / 100));
  u.pitch = Math.min(2, Math.max(0, o.pitchPct / 100));
  await new Promise<void>((resolve) => {
    u.onend = () => resolve();
    u.onerror = () => resolve();
    window.speechSynthesis.speak(u);
  });
}

async function speakCloud(text: string, o: SpeakOptions, seq: number): Promise<void> {
  const client = maybeClient();
  if (!client) throw new Error("not connected");
  const r = await client.call("voice.synthesize", { text, voice: o.cloudVoice, ...(o.style ? { style: o.style } : {}) }, { timeoutMs: 30_000 });
  if (seq !== speakSeq) return;
  const bytes = fromBase64(r.audioBase64);
  const url = URL.createObjectURL(new Blob([bytes.buffer as ArrayBuffer], { type: r.mimeType }));
  currentUrl = url;
  const audio = new Audio(url);
  audio.playbackRate = Math.min(2, Math.max(0.5, o.ratePct / 100));
  currentAudio = audio;
  await new Promise<void>((resolve) => {
    audio.onended = () => resolve();
    audio.onerror = () => resolve();
    audio.play().catch(() => resolve());
  });
  if (currentUrl === url) {
    URL.revokeObjectURL(url);
    currentUrl = null;
  }
}

/**
 * Speaks `text` (interrupting the previous utterance). Cloud TTS falls back
 * to the system voice when the call fails (no consent, no key, offline).
 */
export async function speak(text: string, o: SpeakOptions): Promise<void> {
  const clean = text.trim();
  if (!clean) return;
  stopSpeaking();
  const seq = speakSeq;
  if (o.engine === "OpenAi") {
    try {
      await speakCloud(clean, o, seq);
      return;
    } catch (e) {
      console.warn("cloud TTS failed, using the system voice", e);
      if (seq !== speakSeq) return;
    }
  }
  try {
    await speakSystem(clean, o, seq);
  } catch (e) {
    console.warn("TTS unavailable", e);
  }
}
