/**
 * ADR-0056: the wake listener watchdog (pure state machine), wake phrases
 * in transcripts (several phrases, recognizer misspellings), per-phrase
 * sample storage with the v1 migration, and the follow-up policy.
 */
import { afterEach, describe, expect, it } from "vitest";

import { FollowUpChain, isMeaningfulUtterance } from "../pipeline/followUp";
import { AUDIO_STALL_MS, PHASE_LIMIT_MS, RESTART_BACKOFF_MS, RESUME_GAP_MS, WakeHealth } from "./health";
import { cleanWakePhrases, configWakePhrases, matchWakePhrase, wakeVariants } from "./phrases";
import { loadTemplateSets, pruneTemplateSets, saveTemplatesFor, TEMPLATE_SETS_KEY, TEMPLATES_KEY, templatesFor } from "./spotter";

const LISTENING = { wanted: true, status: "listening", phase: "idle" } as const;

/** Runs checks every 2 s from `from` to `to` (like the controller's timer). */
function run(h: WakeHealth, from: number, to: number, snap: Parameters<WakeHealth["check"]>[1]) {
  const out = [];
  for (let t = from; t <= to; t += 2000) out.push(...h.check(t, snap).map((a) => ({ at: t, ...a })));
  return out;
}

describe("wake listener watchdog", () => {
  it("restarts a listener whose audio stalled, once, then waits for the new stream", () => {
    const h = new WakeHealth();
    h.started(0);
    for (let t = 0; t <= 4000; t += 500) h.beat(t);
    expect(run(h, 2000, 4000, LISTENING)).toEqual([]);
    const acts = run(h, 6000, 4000 + AUDIO_STALL_MS + 2000, LISTENING);
    expect(acts).toHaveLength(1);
    expect(acts[0]).toMatchObject({ type: "restartWake", reason: "stall" });
    expect(h.restarts).toBe(1);
    expect(h.lastReason).toBe("stall");
  });

  it("retries a failed start with growing backoff", () => {
    const h = new WakeHealth();
    const err = { wanted: true, status: "error", phase: "idle" } as const;
    expect(h.check(1000, err)).toEqual([{ type: "restartWake", reason: "error" }]);
    h.failed(1000);
    expect(h.check(1000 + (RESTART_BACKOFF_MS[0] ?? 0) - 1, err)).toEqual([]);
    expect(h.check(1000 + (RESTART_BACKOFF_MS[0] ?? 0), err)).toHaveLength(1);
    h.failed(3000);
    expect(h.check(3000 + (RESTART_BACKOFF_MS[1] ?? 0) - 1, err)).toEqual([]);
    expect(h.check(3000 + (RESTART_BACKOFF_MS[1] ?? 0), err)).toHaveLength(1);
    // Success resets the backoff.
    h.started(10_000);
    h.check(10_001, err);
    expect(h.check(10_002, err)).toHaveLength(1);
  });

  it("restarts after a device change / ended stream and after the computer slept", () => {
    const h = new WakeHealth();
    h.started(0);
    h.beat(1000);
    h.check(1000, LISTENING);
    h.signal("device");
    expect(h.check(2000, LISTENING)).toEqual([{ type: "restartWake", reason: "device" }]);
    h.beat(2500);
    expect(h.check(2600, LISTENING)).toEqual([]);
    expect(h.check(2600 + RESUME_GAP_MS, LISTENING)).toEqual([{ type: "restartWake", reason: "resume" }]);
  });

  it("restarts a start that never finishes, and a wanted listener that is off", () => {
    const h = new WakeHealth();
    const starting = { wanted: true, status: "starting", phase: "idle" } as const;
    expect(run(h, 0, AUDIO_STALL_MS * 3, starting)).toEqual([]);
    expect(h.check(AUDIO_STALL_MS * 3 + 2000, starting)).toEqual([{ type: "restartWake", reason: "stall" }]);
    const h2 = new WakeHealth();
    expect(h2.check(0, { wanted: true, status: "off", phase: "idle" })).toEqual([{ type: "restartWake", reason: "error" }]);
  });

  it("does nothing when the listener is not wanted", () => {
    const h = new WakeHealth();
    h.signal("ended");
    expect(run(h, 0, 60_000, { wanted: false, status: "off", phase: "idle" })).toEqual([]);
  });

  it("resets a phase stuck far past its limit (listening / transcribing / thinking)", () => {
    for (const phase of ["listening", "transcribing", "thinking"] as const) {
      const h = new WakeHealth();
      const limit = PHASE_LIMIT_MS[phase] ?? 0;
      const snap = { wanted: false, status: "off", phase } as const;
      const acts = run(h, 0, limit + 2000, snap);
      expect(acts).toHaveLength(1);
      expect(acts[0]).toMatchObject({ type: "resetPhase", phase });
    }
    // Phase changes restart the clock: a busy but moving pipeline is never reset.
    const h = new WakeHealth();
    let acts: unknown[] = [];
    for (let t = 0; t < 200_000; t += 2000) acts = acts.concat(h.check(t, { wanted: false, status: "off", phase: t % 20_000 < 10_000 ? "listening" : "transcribing" }));
    expect(acts).toEqual([]);
    expect(run(new WakeHealth(), 0, 600_000, { wanted: false, status: "off", phase: "idle" })).toEqual([]);
  });
});

describe("wake phrases in transcripts", () => {
  const phrases = ["Джарвис", "Эй, Джарвис", "Окей, Джарвис", "Jarvis"];

  it("spells every phrase the ways recognizers do", () => {
    const v = wakeVariants(["Джарвис"]);
    for (const s of ["джарвис", "джервис", "жарвис", "дарвис", "jarvis"]) expect(v).toContain(s);
    expect(wakeVariants(["Эй, Джарвис"])).toContain("hey jarvis");
    expect(wakeVariants(["Окей, Джарвис"])).toContain("ok жарвис");
    // Longest first, so «эй джарвис» is stripped before «джарвис».
    const all = wakeVariants(phrases);
    expect(all.indexOf("эй джарвис")).toBeLessThan(all.indexOf("джарвис"));
  });

  it("finds a wake phrase at the start, with the command after it", () => {
    expect(matchWakePhrase("Джервис, открой ютуб", phrases)).toEqual({ heard: "джервис", rest: "открой ютуб" });
    expect(matchWakePhrase("Эй, Жарвис!", phrases)).toEqual({ heard: "эй жарвис", rest: "" });
    expect(matchWakePhrase("Дарвис", phrases)?.rest).toBe("");
    expect(matchWakePhrase("Оркестр, статус", ["Оркестр"])?.rest).toBe("статус");
    expect(matchWakePhrase("Оркестрр статус", ["Оркестр"])?.rest).toBe("статус"); // one-letter slip
    expect(matchWakePhrase("открой ютуб", phrases)).toBeNull();
    expect(matchWakePhrase("", phrases)).toBeNull();
  });

  it("cleans the list from Settings and reads older configs", () => {
    expect(cleanWakePhrases(["  Джарвис ", "джарвис", "", "Эй,  Джарвис"])).toEqual(["Джарвис", "Эй, Джарвис"]);
    expect(cleanWakePhrases([])).toEqual(["Джарвис"]);
    expect(cleanWakePhrases(Array.from({ length: 12 }, (_, i) => `p${i}`))).toHaveLength(8);
    expect(configWakePhrases({ phrase: "Jarvis" })).toEqual(["Jarvis"]);
    expect(configWakePhrases({ phrase: "Jarvis", phrases: [] })).toEqual(["Jarvis"]);
    expect(configWakePhrases({ phrase: "Джарвис", phrases: ["Джарвис", "Jarvis"] })).toEqual(["Джарвис", "Jarvis"]);
  });
});

describe("per-phrase voice samples", () => {
  afterEach(() => {
    window.localStorage.removeItem(TEMPLATES_KEY);
    window.localStorage.removeItem(TEMPLATE_SETS_KEY);
  });

  it("reads the single v1 set, then stores phrases side by side (v2)", () => {
    window.localStorage.setItem(TEMPLATES_KEY, JSON.stringify({ phrase: "Jarvis", samples: [[[1]], [[2]], [[3]]] }));
    expect(loadTemplateSets()).toEqual([{ phrase: "Jarvis", samples: [[[1]], [[2]], [[3]]] }]);
    saveTemplatesFor("Эй, Джарвис", [[[4]], [[5]], [[6]]]);
    expect(window.localStorage.getItem(TEMPLATES_KEY)).toBeNull();
    expect(loadTemplateSets().map((s) => s.phrase)).toEqual(["Jarvis", "Эй, Джарвис"]);
    expect(templatesFor("эй джарвис")?.samples).toHaveLength(3);
    // Re-recording replaces only that phrase; removing a phrase forgets its samples.
    saveTemplatesFor("jarvis", [[[9]], [[9]], [[9]], [[9]]]);
    expect(templatesFor("Jarvis")?.samples).toHaveLength(4);
    pruneTemplateSets(["Jarvis"]);
    expect(loadTemplateSets().map((s) => s.phrase)).toEqual(["jarvis"]);
    saveTemplatesFor("Jarvis", null);
    expect(loadTemplateSets()).toEqual([]);
    expect(window.localStorage.getItem(TEMPLATE_SETS_KEY)).toBeNull();
  });
});

describe("follow-up policy", () => {
  it("ignores noise, fillers and recognizer hallucinations", () => {
    for (const noise of ["", "  ", "э", "Ммм...", "ну", "угу", "а?", "Продолжение следует...", "Субтитры сделал DimaTorzok", "Спасибо за просмотр!", "you", "Hmm"]) {
      expect(isMeaningfulUtterance(noise), noise).toBe(false);
    }
    for (const real of ["а сколько времени", "открой ютуб", "погода", "ещё"]) expect(isMeaningfulUtterance(real), real).toBe(true);
    expect(isMeaningfulUtterance("да")).toBe(false);
    expect(isMeaningfulUtterance("да", { expectingAnswer: true })).toBe(true);
  });

  it("chains at most maxTurns follow-ups, only after real answers", () => {
    const c = new FollowUpChain();
    // Wake-word turn → window; then 3 follow-up turns; the 3rd reply needs the wake word.
    expect(c.afterTurn({ fromFollowUp: false, outcome: "ok" }, 3)).toBe(true);
    expect(c.afterTurn({ fromFollowUp: true, outcome: "ok" }, 3)).toBe(true);
    expect(c.afterTurn({ fromFollowUp: true, outcome: "ok" }, 3)).toBe(true);
    expect(c.afterTurn({ fromFollowUp: true, outcome: "ok" }, 3)).toBe(false);
    // A new wake-word turn starts over.
    expect(c.afterTurn({ fromFollowUp: false, outcome: "ok" }, 3)).toBe(true);
    expect(c.count).toBe(0);
    // Not understood / failed / cancelled: no window.
    for (const outcome of ["notUnderstood", "failed", "cancelled"] as const) expect(c.afterTurn({ fromFollowUp: false, outcome }, 3)).toBe(false);
    // maxTurns 1: one window after the wake-word turn, none after the follow-up turn.
    expect(c.afterTurn({ fromFollowUp: false, outcome: "ok" }, 1)).toBe(true);
    expect(c.afterTurn({ fromFollowUp: true, outcome: "ok" }, 1)).toBe(false);
  });
});
