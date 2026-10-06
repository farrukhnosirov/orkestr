import { describe, expect, it } from "vitest";

import { concatPcm, encodeWav, fromBase64, peak, toBase64 } from "./audio/wav";
import { hotkeyKeys, isHoldEnd, isHoldStart, parseHotkey, yieldsToEditor } from "./hotkey";
import { optionsForProfile, pickVoice, type SpeakOptions, speechLang } from "./tts/speaker";
import { AudioFrontEnd, Resampler } from "./wake/frontend";
import { NUM_CEPS } from "./wake/mfcc";
import { Enrollment } from "./wake/enroll";

describe("wav", () => {
  it("encodes a valid 16-bit mono header and clamps samples", () => {
    const wav = encodeWav(Float32Array.from([0, 1, -1, 2]), 16000);
    const v = new DataView(wav.buffer);
    expect(String.fromCharCode(...wav.subarray(0, 4))).toBe("RIFF");
    expect(String.fromCharCode(...wav.subarray(8, 12))).toBe("WAVE");
    expect(v.getUint32(24, true)).toBe(16000);
    expect(v.getUint32(40, true)).toBe(8);
    expect(v.getInt16(46, true)).toBe(32767);
    expect(v.getInt16(48, true)).toBe(-32768);
    expect(v.getInt16(50, true)).toBe(32767);
  });
  it("concat respects the cap; base64 round-trips; peak", () => {
    const out = concatPcm([Float32Array.from([1, 2]), Float32Array.from([3, 4, 5])], 4);
    expect(Array.from(out)).toEqual([1, 2, 3, 4]);
    const bytes = Uint8Array.from({ length: 70000 }, (_, i) => i % 251);
    expect(Array.from(fromBase64(toBase64(bytes)).subarray(0, 300))).toEqual(Array.from(bytes.subarray(0, 300)));
    expect(fromBase64(toBase64(bytes)).length).toBe(70000);
    expect(peak(Float32Array.from([0.1, -0.7, 0.3]))).toBeCloseTo(0.7);
  });
});

describe("push-to-talk hotkey", () => {
  const h = parseHotkey("Ctrl+Space");
  it("parses and matches", () => {
    expect(h).toEqual({ ctrl: true, shift: false, alt: false, code: "Space" });
    expect(parseHotkey("Ctrl+Shift+V")?.code).toBe("KeyV");
    expect(parseHotkey("F9")?.code).toBe("F9");
    expect(parseHotkey("Ctrl+")).toBeNull();
    expect(parseHotkey("Hyper+X")).toBeNull();
    const ev = { code: "Space", ctrlKey: true, metaKey: false, shiftKey: false, altKey: false };
    expect(h && isHoldStart(ev, h)).toBe(true);
    expect(h && isHoldStart({ ...ev, shiftKey: true }, h)).toBe(false);
    expect(h && isHoldStart({ ...ev, ctrlKey: false }, h)).toBe(false);
  });
  it("releasing the key or the modifier ends the hold", () => {
    expect(h && isHoldEnd({ code: "Space", key: " " }, h)).toBe(true);
    expect(h && isHoldEnd({ code: "ControlLeft", key: "Control" }, h)).toBe(true);
    expect(h && isHoldEnd({ code: "KeyA", key: "a" }, h)).toBe(false);
    expect(hotkeyKeys("Ctrl+Space")).toEqual(["Ctrl", "Space"]);
  });
  it("leaves Ctrl chords to a focused Monaco editor or terminal", () => {
    const host = document.createElement("div");
    host.innerHTML = `<div class="monaco-editor"><textarea id="m"></textarea></div><div class="xterm"><textarea id="x"></textarea></div><input id="plain" />`;
    document.body.appendChild(host);
    const at = (id: string) => host.querySelector(`#${id}`);
    expect(h && yieldsToEditor(at("m"), h)).toBe(true);
    expect(h && yieldsToEditor(at("x"), h)).toBe(true);
    expect(h && yieldsToEditor(at("plain"), h)).toBe(false);
    expect(h && yieldsToEditor(window, h)).toBe(false);
    const f9 = parseHotkey("F9");
    const altChord = parseHotkey("Ctrl+Alt+Space");
    expect(f9 && yieldsToEditor(at("m"), f9)).toBe(false);
    expect(altChord && yieldsToEditor(at("x"), altChord)).toBe(false);
    host.remove();
  });
});

describe("TTS helpers", () => {
  const voices = [
    { name: "Microsoft David", lang: "en-US", localService: true, default: true },
    { name: "Google русский", lang: "ru-RU", localService: false, default: false },
    { name: "Microsoft Irina", lang: "ru-RU", localService: true, default: false },
    { name: "Microsoft Hazel", lang: "en-GB", localService: true, default: false },
  ];
  it("picks the named voice, else a local voice of the same locale", () => {
    expect(pickVoice(voices, "ru-RU", null)?.name).toBe("Microsoft Irina");
    expect(pickVoice(voices, "en-GB", null)?.name).toBe("Microsoft Hazel");
    expect(pickVoice(voices, "en", null)?.name).toBe("Microsoft David");
    expect(pickVoice(voices, "ru-RU", "Google русский")?.name).toBe("Google русский");
    expect(pickVoice(voices, "de-DE", null)).toBeNull();
  });
  it("language resolution and per-agent options", () => {
    expect(speechLang("auto", "ru")).toBe("ru-RU");
    expect(speechLang("en-GB", "ru")).toBe("en-GB");
    const base: SpeakOptions = { engine: "System", lang: "ru-RU", systemVoice: null, cloudVoice: "alloy", style: null, ratePct: 100, pitchPct: 100 };
    const p = optionsForProfile({ provider: "system", voice: "Microsoft Irina", language: "auto", style: null, ratePct: 130 }, base);
    expect(p).toMatchObject({ engine: "System", systemVoice: "Microsoft Irina", ratePct: 130, lang: "ru-RU" });
    // Cloud voice of an agent is only used when cloud TTS is enabled (consent).
    const cloud = optionsForProfile({ provider: "openai", voice: "nova", language: "en", style: "calm" }, base);
    expect(cloud.engine).toBe("System");
    expect(optionsForProfile({ provider: "openai", voice: "nova", language: "en", style: "calm" }, { ...base, engine: "OpenAi" })).toMatchObject({
      engine: "OpenAi",
      cloudVoice: "nova",
      style: "calm",
      lang: "en-US",
    });
    expect(optionsForProfile(null, base)).toBe(base);
  });
});

function tone(freq: number, seconds: number, rate: number, amp: number): Float32Array {
  return Float32Array.from({ length: Math.round(seconds * rate) }, (_, i) => amp * Math.sin((2 * Math.PI * freq * i) / rate) * (0.6 + 0.4 * Math.sin(i / 900)));
}

describe("audio front end", () => {
  it("resamples 48 kHz to 16 kHz (1/3 of the samples, preserving a low tone)", () => {
    const r = new Resampler(48000);
    const out = r.process(tone(300, 1, 48000, 0.5));
    expect(Math.abs(out.length - 16000)).toBeLessThan(3);
    expect(Math.max(...out)).toBeGreaterThan(0.3);
  });

  it("emits one speech segment for a tone burst between silences and reports levels", () => {
    let clock = 0;
    const fe = new AudioFrontEnd({ inputRate: 48000, now: () => clock });
    const blocks: Float32Array[] = [new Float32Array(48000 * 0.6), tone(440, 0.6, 48000, 0.3), new Float32Array(48000 * 0.8)];
    let segments = 0;
    let levels = 0;
    for (const b of blocks) {
      for (let i = 0; i < b.length; i += 128) {
        clock += 0.01;
        const o = fe.process(b.subarray(i, i + 128));
        segments += o.segments.length;
        levels += o.levels.length;
        for (const s of o.segments) expect(s.data.length).toBe(s.frames * NUM_CEPS);
      }
    }
    expect(segments).toBe(1);
    expect(levels).toBeGreaterThan(30);
  });

  it("only produces PCM while recording; economy mode kicks in over budget", () => {
    // Every clock read advances 2.5 ms → 5 ms of "work" per 10 ms block (50 % of a core).
    let clock = 0;
    const fe = new AudioFrontEnd({ inputRate: 16000, now: () => (clock += 2.5), cpuBudgetPct: 1 });
    expect(fe.process(new Float32Array(160)).pcm).toBeNull();
    fe.recording = true;
    expect(fe.process(new Float32Array(160)).pcm?.length).toBe(160);
    for (let i = 0; i < 200; i++) fe.process(new Float32Array(160));
    expect(fe.economy).toBe(true);
    expect(fe.loadPct).toBeGreaterThan(1);
  });
});

describe("enrollment", () => {
  const seg = (frames: number, peakDb: number, shape: number, early = false) => {
    const data = new Float32Array(frames * NUM_CEPS);
    for (let i = 0; i < frames; i++) for (let d = 0; d < NUM_CEPS; d++) data[i * NUM_CEPS + d] = d === 0 ? 20 : 5 * Math.sin((i / frames) * Math.PI * (1 + ((shape * d) % 3)) + shape * d);
    return { data, frames, peakDb, early };
  };
  it("accepts consistent samples up to the maximum and rejects bad ones", () => {
    const e = new Enrollment();
    expect(e.add(seg(10, -20, 1))).toEqual({ ok: false, problem: "tooShort" });
    expect(e.add(seg(60, -60, 1))).toEqual({ ok: false, problem: "tooQuiet" });
    expect(e.add(seg(60, -20, 1, true))).toEqual({ ok: false, problem: "tooLong" });
    expect(e.add(seg(60, -20, 1))).toMatchObject({ ok: true, count: 1 });
    expect(e.add(seg(62, -20, 1))).toMatchObject({ ok: true, count: 2 });
    expect(e.complete).toBe(false);
    expect(e.add(seg(58, -20, 1))).toMatchObject({ ok: true, count: 3 });
    expect(e.complete).toBe(true);
    expect(e.add(seg(60, -20, 2.6))).toEqual({ ok: false, problem: "inconsistent" });
    e.add(seg(61, -20, 1));
    e.add(seg(59, -20, 1));
    expect(e.full).toBe(true);
    expect(e.add(seg(60, -20, 1))).toEqual({ ok: false, problem: "full" });
  });
});
