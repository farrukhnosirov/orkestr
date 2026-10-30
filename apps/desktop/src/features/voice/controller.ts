/**
 * Voice command controller (§33, §63 G, ADR-0017): push-to-talk / wake word
 * → speech-to-text → local intent parser → command execution → spoken reply.
 *
 * Privacy (§33): the microphone opens only for (a) local wake-word spotting
 * (audio never leaves the device; templates live in this webview), (b) a
 * command after push-to-talk or the wake word, sent to the consented STT
 * engine, or (c) enrollment / the level meter in Settings. The mic indicator
 * in the title bar shows every open purpose. Audio buffers are dropped right
 * after recognition; only transcripts are logged (`voice.recordCommand`).
 *
 * Barge-in and continuous conversation (ADR-0054): while the assistant
 * speaks or works, the same microphone feeds the core's stop-phrase spotter
 * (`barge/spotter.ts`); «стоп» / «хватит» silence speech and cancel the
 * reply or action, «окей» / «тихо» only silence speech, then a short window
 * listens for the next command. After a reply the assistant re-opens
 * listening without the wake word for the follow-up window; silence, «спасибо,
 * всё» or a stop phrase end the conversation quietly.
 */
import type { BargeHit, Event, StopKind, VoiceCommandSource } from "@orkestr/protocol";
import { topic } from "@orkestr/protocol";

import { maybeClient } from "../../core/client";
import { currentLanguage, t } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { agentName } from "../../lib/coreText";
import { useAppStore } from "../../store/app";
import { usePerfStore } from "../performance/perfStore";
import { useSettingsStore } from "../settings/settingsStore";
import { playSound } from "../sound/engine";
import { assistantOptions } from "./agentVoices";
import type { MicCapture } from "./audio/mic";
import { type Intent, intentId, needsConfirmation, parseConfirmation, parseIntent } from "./intent/parser";
import { realVoiceDeps } from "./pipeline/deps";
import { bindApproval, type CommandResult, confirmationPrompt, executeIntent, precheck, type VoiceDeps } from "./pipeline/executor";
import { assistantResult } from "../assistant/voiceResult";
import { isWeakIntent, routeUtterance } from "../assistant/voiceRoute";
import { dailyGreeting } from "../jarvis/greeting";
import { isBusy, type MachineEvent, nextPhase } from "./pipeline/machine";
import { speakable } from "./pipeline/summary";
import { type MicPurpose, useVoiceStore } from "./store";
import { engineNeedsPcm, SttError, type SttSession, startStt, webSpeechAvailable } from "./stt/engines";
import { speak, speechLang, stopSpeaking } from "./tts/speaker";
import { confirmPending, stopAssistant } from "../assistant/api";
import { useAssistantStore } from "../assistant/store";
import { stopCursor, useCursorStore } from "../cursor/store";
import { jarvisSession } from "../jarvis/session";
import { classifyStop, isConversationEnd, isEchoOf, splitLeadingStop } from "./barge/phrases";
import { type BargeTransport, BargeSpotterClient } from "./barge/spotter";
import { segmentFrames } from "./wake/segmenter";
import { loadTemplates as loadWakeTemplates, WakeSpotter } from "./wake/spotter";

let deps: VoiceDeps | null = null;
function getDeps(): VoiceDeps {
  deps ??= realVoiceDeps();
  return deps;
}

/** Test seam. */
export function setVoiceDeps(d: VoiceDeps | null): void {
  deps = d;
}

// ---------------------------------------------------------------------------
// Microphone (shared by wake spotting, commands, enrollment, meter).
// ---------------------------------------------------------------------------

let mic: MicCapture | null = null;
/** Messages received from the microphone worklet (debug: proves the loop runs in the background). */
let micTicks = 0;
let micOpening: Promise<MicCapture> | null = null;
let offLevel: (() => void) | null = null;

function emit(ev: MachineEvent): boolean {
  const s = useVoiceStore.getState();
  const next = nextPhase(s.phase, ev);
  if (next === null) return false;
  s.setRuntime({ phase: next });
  return true;
}

async function openMic(purpose: MicPurpose): Promise<MicCapture> {
  const store = useVoiceStore.getState();
  store.micOn(purpose);
  if (mic && !mic.isClosed) return mic;
  if (!micOpening) {
    const cfg = store.config;
    micOpening = import("./audio/mic")
      .then(({ MicCapture: Mic }) => Mic.open({ deviceId: cfg?.inputDeviceId ?? null, cpuBudgetPct: cfg?.wake.cpuBudgetPct ?? 2 }))
      .then(
        (m) => {
          mic = m;
          micOpening = null;
          let smoothed = 0;
          offLevel = m.subscribe((msg) => {
            micTicks += 1;
            if (msg.type === "level") {
              // dBFS → 0..1 (−60 dB .. −10 dB), fast attack / slow release.
              const v = Math.min(1, Math.max(0, (msg.db + 60) / 50));
              smoothed = v > smoothed ? v : smoothed * 0.8 + v * 0.2;
              useVoiceStore.getState().setRuntime({ level: smoothed });
            } else if (msg.type === "load") {
              useVoiceStore.getState().setRuntime({ wakeLoadPct: msg.pct });
            }
          });
          // Purposes may have been released while the device was opening.
          if (useVoiceStore.getState().mic.length === 0) closeMic();
          return m;
        },
        (e: unknown) => {
          micOpening = null;
          useVoiceStore.getState().micOff(purpose);
          throw e;
        },
      );
  }
  return micOpening;
}

function closeMic(): void {
  offLevel?.();
  offLevel = null;
  mic?.close();
  mic = null;
}

function releaseMic(purpose: MicPurpose): void {
  const store = useVoiceStore.getState();
  store.micOff(purpose);
  if (useVoiceStore.getState().mic.length === 0) closeMic();
}

/** The live analyser of the open microphone (HUD waveform), if any. */
export function currentAnalyser(): AnalyserNode | null {
  return mic && !mic.isClosed ? mic.analyser : null;
}

/** Subscribes to the mic (Settings meter / enrollment). Returns the capture and a release function. */
export async function acquireMic(purpose: "enroll" | "meter"): Promise<{ mic: MicCapture; release: () => void }> {
  const m = await openMic(purpose);
  return { mic: m, release: () => releaseMic(purpose) };
}

export function micErrorText(e: unknown): string {
  const code = (e as { code?: string } | null)?.code;
  if (code === "denied") return t("voice:errors.micDenied");
  if (code === "notFound") return t("voice:errors.micNotFound");
  if (code === "unsupported") return t("voice:errors.micUnsupported");
  return t("voice:errors.micFailed");
}

// ---------------------------------------------------------------------------
// Command capture.
// ---------------------------------------------------------------------------

let session: SttSession | null = null;
let sessionSource: VoiceCommandSource | null = null;
/**
 * Generation of the command capture being started. `stopListening` (key
 * released) and `cancelVoice` bump it while the microphone is still opening,
 * so a capture whose trigger already ended never starts recording.
 */
let startGen = 0;
let starting = false;
let pending: Intent | null = null;
let dismissTimer: number | null = null;
let managerWatch: (() => void) | null = null;
/** The capture in progress is a follow-up window (no wake word): silence closes it quietly. */
let followUpCapture = false;
/** Turn counter: a turn interrupted by a hard stop phrase must not speak or show its late result. */
let turnSeq = 0;
/** Speech of the current turn was silenced (barge-in): later sentences are dropped. */
let hushed = false;
/** `say()` calls in progress. */
let speechDepth = 0;
/** What the assistant is saying now / said last (self-echo protection). */
let currentSpoken: string | null = null;
let lastSpoken = "";
/** A reply finished: re-open listening for a follow-up once speech is over. */
let followUpArmed = false;
let followUpTimer: number | null = null;

/** Pause between the end of the assistant's speech and the follow-up window (room echo tail). */
export const FOLLOW_UP_DELAY_MS = 300;
/** Listening window after a stop phrase («стоп, открой ютуб»). */
export const AFTER_STOP_MS = 5000;

function clearDismiss(): void {
  if (dismissTimer !== null) window.clearTimeout(dismissTimer);
  dismissTimer = null;
}

function scheduleDismiss(ms: number): void {
  clearDismiss();
  dismissTimer = window.setTimeout(() => {
    const s = useVoiceStore.getState();
    if (s.phase === "result" || s.phase === "error") {
      pending = null; // an offered follow-up expires with the reply
      emit({ type: "dismiss" });
      if (!s.hudOpen) s.setRuntime({ transcript: "", reply: null });
    }
  }, ms);
}

function lang(): string {
  return speechLang(useVoiceStore.getState().config?.language ?? "auto", currentLanguage());
}

/** Whether a speech engine can run now (selected, consented, available). */
export function sttReady(): boolean {
  const c = useVoiceStore.getState().config;
  if (!c || !c.enabled || c.sttEngine === "Disabled") return false;
  if (c.sttEngine === "WebSpeech") return webSpeechAvailable();
  return true;
}

export interface ListenOptions {
  /** A follow-up window (no wake word): close quietly when nothing is said for this long, ms. */
  followUpMs?: number;
  /** Audio captured just before (barge-in pre-roll). */
  preroll?: readonly Float32Array[];
}

/** Push-to-talk pressed / HUD mic clicked / wake word heard / follow-up window. */
export async function startListening(source: VoiceCommandSource, opts: ListenOptions = {}): Promise<void> {
  const s = useVoiceStore.getState();
  const c = s.config;
  if (!c?.enabled) return;
  if (session) return;
  // Push-to-talk / the wake word while the assistant works: interrupt it and take the new command.
  if (isBusy(s.phase)) {
    if (s.phase !== "thinking" || source === "Typed" || !bargeEnabled()) return;
    interrupt("Hard", { listen: false });
  }
  clearDismiss();
  clearFollowUp();
  stopBarge();
  stopSpeaking();
  hushed = false;
  if (!sttReady()) {
    // No recognition: open the HUD for a typed command.
    s.setHudOpen(true);
    s.setRuntime({ source: "Typed" });
    // Say why there is no listening (instead of a generic "unavailable").
    if (c.sttEngine === "Disabled" && source !== "Typed") s.setRuntime({ reply: { text: t("voice:errors.sttDisabled"), tone: "info", outcome: "ok" } });
    return;
  }
  if (!emit({ type: "listen" })) return;
  const follow = opts.followUpMs !== undefined;
  followUpCapture = follow;
  s.setRuntime({
    source,
    transcript: "",
    reply: pending ? s.reply : null,
    followUp: follow ? { until: Date.now() + (opts.followUpMs ?? 0), ms: opts.followUpMs ?? 0 } : null,
  });
  if (source !== "WakeWord" && !follow) playSound("listenStart");
  const gen = ++startGen;
  starting = true;
  try {
    const needPcm = engineNeedsPcm(c.sttEngine);
    const m = needPcm ? await openMic("command") : null;
    if (gen !== startGen) {
      // Released (or cancelled) before the microphone was ready: record nothing.
      releaseMic("command");
      if (useVoiceStore.getState().phase === "listening") emit({ type: "cancel" });
      return;
    }
    if (!needPcm) useVoiceStore.getState().micOn("command");
    session = startStt({
      engine: c.sttEngine,
      lang: lang(),
      mic: m,
      autoEnd: source === "WakeWord" || follow,
      ...(follow ? { noSpeechMs: opts.followUpMs } : {}),
      ...(opts.preroll?.length ? { preroll: opts.preroll } : {}),
    });
    sessionSource = source;
    session.onInterim((text) => useVoiceStore.getState().setRuntime({ transcript: text }));
    session.onAutoEnd(() => void stopListening());
    // The user started talking: the follow-up window extends until they finish.
    session.onSpeech?.(() => useVoiceStore.getState().setRuntime({ followUp: null }));
  } catch (e) {
    releaseMic("command");
    session = null;
    if (gen === startGen) fail(e instanceof SttError ? sttErrorText(e) : micErrorText(e));
  } finally {
    if (gen === startGen) starting = false;
  }
}

function sttErrorText(e: SttError): string {
  switch (e.code) {
    case "denied":
      return t("voice:errors.micDenied");
    case "network":
      return t("voice:errors.sttNetwork");
    case "noSpeech":
      return t("voice:errors.noSpeech");
    case "unsupported":
      return t("voice:errors.sttUnsupported");
    case "disabled":
      return t("voice:errors.sttDisabled");
    case "noModel":
      return t("voice:errors.sttNoModel");
    default:
      return t("voice:errors.sttFailed");
  }
}

function fail(text: string): void {
  const s = useVoiceStore.getState();
  emit({ type: "fail" });
  s.setRuntime({ reply: { text, tone: "error", outcome: "failed" } });
  playSound("error");
  scheduleDismiss(7000);
}

/** Push-to-talk released (or silence after the wake word). */
export async function stopListening(): Promise<void> {
  const sess = session;
  if (!sess) {
    // Released while the microphone is still opening: abandon that capture.
    if (starting) {
      startGen += 1;
      starting = false;
    }
    return;
  }
  session = null;
  const source = sessionSource ?? "PushToTalk";
  const follow = followUpCapture;
  followUpCapture = false;
  useVoiceStore.getState().setRuntime({ followUp: null });
  if (!emit({ type: "speechEnd" })) {
    sess.cancel();
    releaseMic("command");
    return;
  }
  playSound("listenEnd");
  let text = "";
  try {
    text = await sess.finish();
  } catch (e) {
    releaseMic("command");
    if (follow && e instanceof SttError && (e.code === "noSpeech" || e.code === "rejected")) return closeConversation();
    // Noise / music / TV (ADR-0057): stay quiet, at most «не расслышал» in the HUD.
    if (e instanceof SttError && e.code === "rejected") return notHeard();
    fail(e instanceof SttError ? sttErrorText(e) : describeError(e));
    return;
  }
  releaseMic("command");
  if (follow) {
    const next = followUpText(text);
    if (next === null) return closeConversation();
    text = next;
  }
  if (!text) {
    fail(t("voice:errors.noSpeech"));
    return;
  }
  useVoiceStore.getState().setRuntime({ transcript: text });
  emit({ type: "transcript", text });
  sttConfidence = sess.confidence ? sess.confidence() : null;
  sttFollowUp = follow;
  try {
    await handleText(text, source);
  } finally {
    sttConfidence = undefined;
    sttFollowUp = false;
  }
}

/** Recognizer confidence of the utterance being handled (ADR-0057; `undefined`: typed / simulated). */
let sttConfidence: number | null | undefined;
/** The utterance being handled came in the follow-up window (no wake word). */
let sttFollowUp = false;

/** The recording was noise or too unsure to act on: no sound, no speech, a quiet HUD note (ADR-0057). */
function notHeard(): void {
  emit({ type: "cancel" });
  useVoiceStore.getState().setRuntime({ followUp: null, transcript: "", reply: { text: t("voice:errors.notHeard"), tone: "info", outcome: "cancelled" } });
  scheduleDismiss(2500);
}

/** Cancels whatever the pipeline is doing (Esc, HUD close). */
export function cancelVoice(): void {
  startGen += 1;
  starting = false;
  session?.cancel();
  session = null;
  pending = null;
  followUpCapture = false;
  followUpArmed = false;
  clearFollowUp();
  releaseMic("command");
  stopSpeaking();
  clearDismiss();
  emit({ type: "cancel" });
  useVoiceStore.getState().setRuntime({ confirmText: null, transcript: "", reply: null, followUp: null });
}

/** A typed command from the HUD. */
export async function submitTyped(text: string): Promise<void> {
  const clean = text.trim();
  if (!clean) return;
  clearDismiss();
  clearFollowUp();
  stopSpeaking();
  hushed = false;
  if (!emit({ type: "typed", text: clean })) return;
  useVoiceStore.getState().setRuntime({ source: "Typed", transcript: clean, reply: null });
  await handleText(clean, "Typed");
}

/**
 * Acceptance seam (§63 G without microphone hardware): runs a transcript
 * through exactly the path a finished speech recognition takes
 * (listen → speechEnd → transcript → intent → execute → spoken reply).
 * No audio is opened. Exposed on `window.__orkestrDebug` only when the
 * `orkestr.debugHooks` local-storage flag is set (see `installVoiceDebugHook`).
 */
export async function simulateTranscript(text: string, source: VoiceCommandSource = "PushToTalk"): Promise<void> {
  const clean = text.trim();
  if (!clean || !useVoiceStore.getState().config?.enabled) return;
  clearDismiss();
  stopSpeaking();
  if (!emit({ type: "listen" })) return;
  useVoiceStore.getState().setRuntime({ source, transcript: "", reply: null });
  emit({ type: "speechEnd" });
  useVoiceStore.getState().setRuntime({ transcript: clean });
  emit({ type: "transcript", text: clean });
  await handleText(clean, source);
}

/** Installs `window.__orkestrDebug.voice` when the debug-hooks flag is on. */
export function installVoiceDebugHook(): () => void {
  let on = false;
  try {
    on = window.localStorage.getItem("orkestr.debugHooks") === "1";
  } catch {
    on = false;
  }
  if (!on) return () => undefined;
  const w = window as unknown as { __orkestrDebug?: Record<string, unknown> };
  w.__orkestrDebug = {
    ...(w.__orkestrDebug ?? {}),
    voice: {
      simulateTranscript,
      /** Same path as a detected wake word (sound + command capture). */
      simulateWake: () => onWake(),
      cancel: () => cancelVoice(),
      /** Same path as a spotted stop phrase (`Hard` / `Soft`). */
      simulateStop: (kind: StopKind = "Hard") => interrupt(kind),
      state: () => {
        const s = useVoiceStore.getState();
        return {
          enabled: !!s.config?.enabled,
          phase: s.phase,
          transcript: s.transcript,
          reply: s.reply,
          speaking: s.speaking,
          hudOpen: s.hudOpen,
          wakeStatus: s.wakeStatus,
          followUp: s.followUp,
          interruptedAt: s.interruptedAt,
          barge: barge?.engine ?? null,
          mic: [...s.mic],
          micTicks,
          hidden: document.hidden,
        };
      },
    },
  };
  return () => {
    if (w.__orkestrDebug) delete w.__orkestrDebug.voice;
  };
}

/** Confirmation via the HUD buttons. */
export async function answerConfirmation(yes: boolean): Promise<void> {
  if (useVoiceStore.getState().phase !== "confirm" || !pending) return;
  const intent = pending;
  pending = null;
  useVoiceStore.getState().setRuntime({ confirmText: null });
  if (!yes) {
    finish(intent, { outcome: "cancelled", reply: t("voice:reply.cancelled"), tone: "info", projectId: null }, "Typed");
    return;
  }
  emit({ type: "confirmed" });
  await run(intent, useVoiceStore.getState().source ?? "Typed");
}

function wakePhrases(): string[] {
  const c = useVoiceStore.getState().config;
  return c ? [c.wake.phrase] : [];
}

async function intentContext() {
  const app = useAppStore.getState();
  let agents: { id: string; name: string; aliases: string[]; projectId: string }[] = [];
  try {
    // The localized name (what the UI shows, «Тестировщик #1») is matched alongside the stored one.
    agents = (await getDeps().listAgents(null)).map((a) => {
      const shown = agentName(a.name);
      return { id: a.id, name: a.name, aliases: shown !== a.name ? [shown] : [], projectId: a.projectId };
    });
  } catch {
    /* agent names are optional for parsing */
  }
  return {
    projects: app.projects.map((p) => ({ id: p.id, name: p.name })),
    agents,
    activeProjectId: app.activeProjectId,
    wakePhrases: wakePhrases(),
  };
}

async function handleText(text: string, source: VoiceCommandSource): Promise<void> {
  const s = useVoiceStore.getState();
  const turn = ++turnSeq;
  hushed = false;
  // An answer to a pending confirmation?
  if (pending) {
    const answer = parseConfirmation(text, wakePhrases());
    if (answer) {
      const intent = pending;
      pending = null;
      s.setRuntime({ confirmText: null });
      if (answer === "no") {
        finish(intent, { outcome: "cancelled", reply: t("voice:reply.cancelled"), tone: "info", projectId: null }, source);
        return;
      }
      await run(intent, source);
      return;
    }
    pending = null; // something else was said: treat it as a new command
    s.setRuntime({ confirmText: null });
  }
  // Approve/reject are bound to the approval read out in the prompt (exact id).
  const intent = bindApproval(parseIntent(text, await intentContext()), getDeps());
  // Stop phrases first (ADR-0054): «хватит» / «замолчи» never reach the command
  // matcher or its «Вы имели в виду» near-miss confirmation (ADR-0055). A strong
  // Orkestr intent («стоп всё», «останови проект X») still wins.
  const stop = source !== "Typed" && s.config && isWeakIntent(intent) ? classifyStop(text, s.config.barge) : null;
  if (stop) {
    if (useAssistantStore.getState().pending) await confirmPending(false).catch(() => null);
    interrupt(stop.kind, { listen: false });
    finish(intent, { outcome: "cancelled", reply: t("voice:reply.cancelled"), tone: "info", projectId: null }, source);
    return;
  }
  // Computer assistant (ADR-0035): computer commands, its confirmations and AI plans.
  // Jarvis (ADR-0049): AI conversation replies stream into the HUD and are spoken sentence by sentence.
  const viaAssistant = await routeUtterance(text, intent, wakePhrases(), {
    ...(source !== "Typed" && sttConfidence !== undefined ? { confidence: sttConfidence, followUp: sttFollowUp } : {}),
    speak: (sentence) => say(sentence),
    onPartial: (partial) => {
      if (partial) useVoiceStore.getState().setRuntime({ reply: { text: partial, tone: "info", outcome: "ok" } });
    },
  }).catch(() => null);
  if (turn !== turnSeq) return; // interrupted by a stop phrase while it ran
  if (viaAssistant?.unheard) {
    notHeard();
    return;
  }
  if (viaAssistant) {
    finish(intent, assistantResult(viaAssistant), source);
    for (const extra of viaAssistant.speak) void say(extra);
    return;
  }
  const pre = precheck(intent, getDeps());
  if (pre) {
    finish(intent, pre, source);
    return;
  }
  if (needsConfirmation(intent)) {
    pending = intent;
    const prompt = confirmationPrompt(intent, getDeps());
    emit({ type: "needsConfirm" });
    s.setRuntime({ confirmText: prompt, reply: null });
    playSound("approval");
    if (source !== "Typed") followUpArmed = true; // the spoken answer needs no wake word
    void say(`${prompt} ${t("voice:confirm.sayYesNo")}`);
    if (speechDepth === 0) scheduleFollowUp();
    void record(text, intent, "needsConfirmation", source, null);
    return;
  }
  await run(intent, source);
}

async function run(intent: Intent, source: VoiceCommandSource): Promise<void> {
  const turn = turnSeq;
  let r: CommandResult;
  try {
    r = await executeIntent(intent, getDeps());
  } catch (e) {
    r = { outcome: "failed", reply: t("voice:reply.failed", { error: describeError(e) }), tone: "error", projectId: null };
  }
  if (turn !== turnSeq) return;
  finish(intent, r, source);
}

function finish(intent: Intent, r: CommandResult, source: VoiceCommandSource): void {
  const s = useVoiceStore.getState();
  if (s.phase === "thinking") emit({ type: "done" });
  else if (s.phase === "confirm") {
    emit({ type: "confirmed" });
    emit({ type: "done" });
  } else s.setRuntime({ phase: "result" });
  // first request of the day: «Доброе утро, сэр.» before the reply (Jarvis setting)
  const hello = r.outcome !== "cancelled" && !r.spoken ? dailyGreeting() : null;
  const reply = hello ? `${hello} ${r.reply}` : r.reply;
  s.setRuntime({ reply: { text: reply, tone: r.tone, outcome: r.outcome } });
  s.pushHistory({ transcript: s.transcript, reply, tone: r.tone, source });
  playSound(r.tone === "error" ? "error" : r.outcome === "cancelled" ? "listenEnd" : "completion");
  if (!r.spoken) void say(reply);
  void record(s.transcript, intent, r.outcome, source, r.projectId);
  if (r.managerReply) watchManagerReply(r.managerReply.projectId, r.managerReply.messageId);
  // An offered next step ("Show the errors?"): a spoken "yes" runs it.
  if (r.followUp) pending = r.followUp;
  scheduleDismiss(r.managerReply ? 60_000 : r.followUp ? 15_000 : 9000);
  // Continuous conversation: listen for the next utterance once the reply was spoken.
  if (source !== "Typed" && r.outcome !== "cancelled") {
    followUpArmed = true;
    if (speechDepth === 0) scheduleFollowUp();
  }
}

/** Says the current reply again (Jarvis card "Repeat"). Keeps the card open while it speaks. */
export function repeatReply(): void {
  const s = useVoiceStore.getState();
  const text = s.reply?.text ?? s.confirmText;
  if (!text) return;
  if (s.phase === "result" || s.phase === "error") scheduleDismiss(9000);
  void say(text);
}

/** Speaks a reply with the active TTS engine (when spoken replies are on). */
export async function say(text: string): Promise<void> {
  const c = useVoiceStore.getState().config;
  const opts = assistantOptions();
  if (!c?.spokenReplies || !opts || hushed) return;
  const line = speakable(text, 260);
  speechDepth += 1;
  currentSpoken = line;
  lastSpoken = line;
  useVoiceStore.getState().setRuntime({ speaking: true });
  try {
    await speak(line, opts);
  } finally {
    speechDepth = Math.max(0, speechDepth - 1);
    if (speechDepth === 0) {
      currentSpoken = null;
      useVoiceStore.getState().setRuntime({ speaking: false });
      scheduleFollowUp();
    }
  }
}

// ---------------------------------------------------------------------------
// Barge-in (ADR-0054).
// ---------------------------------------------------------------------------

/** What barge-in may cancel (test seam). */
export interface BargeActions {
  /** Stops the AI reply being streamed. */
  cancelReply: () => Promise<void>;
  /** A computer command (multi-step) is running. */
  assistantBusy: () => boolean;
  stopAssistant: () => Promise<void>;
  /** Cursor capture mode is running. */
  cursorActive: () => boolean;
  stopCursor: () => Promise<void>;
}

const realBargeActions: BargeActions = {
  cancelReply: () => jarvisSession.cancel(),
  assistantBusy: () => useAssistantStore.getState().busy,
  stopAssistant: () => stopAssistant(),
  cursorActive: () => useCursorStore.getState().active,
  stopCursor: async () => {
    await stopCursor();
  },
};
let bargeActions: BargeActions = realBargeActions;

function coreBargeTransport(): BargeTransport {
  return {
    start: async () => {
      const client = maybeClient();
      if (!client) return { sessionId: null, engine: null, reason: "offline" };
      return client.call("voice.bargeStart", {});
    },
    feed: async (sessionId, pcmBase64, spoken) => {
      const client = maybeClient();
      if (!client) throw new Error("offline");
      return client.call("voice.bargeFeed", { sessionId, pcmBase64, ...(spoken ? { spoken } : {}) });
    },
    stop: async (sessionId) => {
      await maybeClient()?.call("voice.bargeStop", { sessionId });
    },
  };
}
let bargeTransport: BargeTransport | null = null;

/** Test seams. */
export function setBargeTransport(t: BargeTransport | null): void {
  bargeTransport = t;
}
export function setBargeActions(a: BargeActions | null): void {
  bargeActions = a ?? realBargeActions;
}

let barge: BargeSpotterClient | null = null;
let bargeGen = 0;
/** A start in flight (opening the mic / the core session). */
let bargeStarting = false;
/** No offline engine could spot phrases: do not retry before this time (epoch ms). */
let bargeRetryAt = 0;

function bargeEnabled(): boolean {
  const c = useVoiceStore.getState().config;
  return !!c?.enabled && !!c.barge?.enabled;
}

/** Whether the spotter should run now: the assistant speaks, works, or controls the cursor. */
export function bargeWanted(): boolean {
  if (!bargeEnabled()) return false;
  const s = useVoiceStore.getState();
  if (s.phase === "listening" || s.phase === "transcribing" || session || s.mic.includes("enroll")) return false;
  return s.speaking || s.phase === "thinking" || bargeActions.cursorActive();
}

/** Starts or stops the stop-phrase spotter to match the state. Idempotent. */
export async function syncBarge(): Promise<void> {
  const want = bargeWanted();
  if (!want) {
    stopBarge();
    return;
  }
  if (barge?.running || bargeStarting || Date.now() < bargeRetryAt) return;
  const gen = ++bargeGen;
  barge = null;
  bargeStarting = true;
  try {
    const m = await openMic("barge");
    if (gen !== bargeGen || !bargeWanted()) {
      if (gen === bargeGen) releaseMic("barge");
      return;
    }
    const client = await BargeSpotterClient.start({
      transport: bargeTransport ?? coreBargeTransport(),
      mic: m,
      spoken: () => currentSpoken,
      onHit: (hit, preroll) => onBargeHit(hit, preroll),
      // A command capture that started meanwhile keeps recording.
      releaseRecording: () => {
        if (!session) m.setRecording(false);
      },
    });
    if (!client) {
      bargeRetryAt = Date.now() + 30_000;
      if (gen === bargeGen) releaseMic("barge");
      return;
    }
    if (gen !== bargeGen || !bargeWanted()) {
      client.stop();
      if (gen === bargeGen) releaseMic("barge");
      return;
    }
    barge = client;
  } catch {
    if (gen === bargeGen) releaseMic("barge");
    bargeRetryAt = Date.now() + 30_000;
  } finally {
    if (gen === bargeGen) bargeStarting = false;
  }
}

function stopBarge(): void {
  bargeGen += 1;
  bargeStarting = false;
  const b = barge;
  barge = null;
  b?.stop();
  if (useVoiceStore.getState().mic.includes("barge")) releaseMic("barge");
}

function onBargeHit(hit: BargeHit, preroll: Float32Array[]): void {
  barge = null;
  releaseMic("barge");
  // Final self-echo guard on the client (the core checks the sentence being spoken too).
  const c = useVoiceStore.getState().config;
  if (c && classifyStop(hit.heard, c.barge, currentSpoken) === null && classifyStop(hit.phrase, c.barge, currentSpoken) === null) return;
  interrupt(hit.kind, { preroll });
}

/**
 * Interrupts the assistant: silences speech at once; a hard stop (or a soft
 * one with "soft phrases cancel actions") also cancels the streamed reply,
 * the running computer command and cursor control. Nothing is said
 * («Слушаю, сэр.» is not spoken); the circle plays the "interrupted"
 * animation and listens for the next command for a few seconds.
 */
export function interrupt(kind: StopKind, o: { listen?: boolean; preroll?: readonly Float32Array[] } = {}): void {
  const s = useVoiceStore.getState();
  const c = s.config;
  stopBarge();
  clearFollowUp();
  followUpArmed = false;
  hushed = true;
  stopSpeaking();
  speechDepth = 0;
  currentSpoken = null;
  const cancelActions = kind === "Hard" || !!c?.barge.softCancelsActions;
  const cursor = bargeActions.cursorActive();
  if (cursor) void bargeActions.stopCursor().catch(() => undefined);
  if (cancelActions) {
    turnSeq += 1; // the turn in flight neither speaks nor shows its late result
    void bargeActions.cancelReply().catch(() => undefined);
    if (bargeActions.assistantBusy()) void bargeActions.stopAssistant().catch(() => undefined);
    pending = null;
    if (s.phase === "thinking" || s.phase === "confirm") emit({ type: "cancel" });
    s.setRuntime({ confirmText: null });
  }
  s.setRuntime({ speaking: false, interruptedAt: Date.now() });
  const listen = o.listen ?? true;
  if (listen && !isBusy(useVoiceStore.getState().phase) && sttReady()) {
    void startListening("WakeWord", { followUpMs: AFTER_STOP_MS, ...(o.preroll ? { preroll: o.preroll } : {}) });
  }
}

/** Esc / a click on the circle while it speaks: silence speech only (a running action goes on). */
export function hushSpeech(): void {
  if (!useVoiceStore.getState().speaking) return;
  interrupt("Soft", { listen: false });
}

// ---------------------------------------------------------------------------
// Continuous conversation (ADR-0054).
// ---------------------------------------------------------------------------

function clearFollowUp(): void {
  if (followUpTimer !== null) window.clearTimeout(followUpTimer);
  followUpTimer = null;
}

/** After a reply: re-open listening (no wake word) for the follow-up window. */
function scheduleFollowUp(): void {
  if (!followUpArmed || speechDepth > 0) return;
  followUpArmed = false;
  const c = useVoiceStore.getState().config;
  if (!c?.followUp?.enabled || !sttReady()) return;
  clearFollowUp();
  followUpTimer = window.setTimeout(() => {
    followUpTimer = null;
    const s = useVoiceStore.getState();
    if ((s.phase !== "result" && s.phase !== "confirm") || s.speaking || session) return;
    void startListening("WakeWord", { followUpMs: c.followUp.windowSecs * 1000 });
  }, FOLLOW_UP_DELAY_MS);
}

/**
 * A follow-up utterance: `null` ends the conversation (a stop phrase alone,
 * «спасибо, всё», the assistant's own echo), otherwise the command to run
 * («стоп, открой ютуб» → «открой ютуб»).
 */
export function followUpText(text: string): string | null {
  const c = useVoiceStore.getState().config;
  const clean = text.trim();
  if (!clean) return null;
  if (isEchoOf(clean, lastSpoken)) return null;
  if (isConversationEnd(clean)) return null;
  if (c) {
    const lead = splitLeadingStop(clean, c.barge);
    if (lead) return lead.rest && !isConversationEnd(lead.rest) ? lead.rest : null;
    if (classifyStop(clean, c.barge)) return null;
  }
  return clean;
}

/** Silence / an ending in the follow-up window: close without a word. */
function closeConversation(): void {
  const s = useVoiceStore.getState();
  emit({ type: "cancel" });
  pending = null;
  s.setRuntime({ followUp: null, confirmText: null, transcript: "", reply: null });
  if (s.config?.followUp.chimeOnClose) playSound("listenEnd");
}

async function record(transcript: string, intent: Intent, outcome: string, source: VoiceCommandSource, projectId: string | null): Promise<void> {
  const client = maybeClient();
  if (!client || !transcript) return;
  try {
    await client.call("voice.recordCommand", {
      transcript: transcript.slice(0, 500),
      intent: intentId(intent),
      outcome,
      source,
      ...(projectId ? { projectId } : {}),
    });
  } catch (e) {
    console.warn("voice.recordCommand failed", e);
  }
}

/** Speaks the Manager's reply to a voice question when it arrives (≤ 2 min). */
function watchManagerReply(projectId: string, messageId: string): void {
  managerWatch?.();
  const client = maybeClient();
  if (!client) return;
  const timer = window.setTimeout(() => stop(), 120_000);
  const off = client.subscribe([topic.project(projectId)], (ev: Event) => {
    if (ev.kind !== "ManagerMessageAdded") return;
    const m = ev.payload.message;
    if (m.id !== messageId || !m.text.trim()) return;
    stop();
    const text = speakable(m.text, 400);
    const s = useVoiceStore.getState();
    s.setRuntime({ reply: { text, tone: "info", outcome: "ok" } });
    if (s.phase === "idle") s.setRuntime({ phase: "result" });
    void say(text);
    scheduleDismiss(15_000);
  });
  function stop() {
    window.clearTimeout(timer);
    off();
    if (managerWatch === stop) managerWatch = null;
  }
  managerWatch = stop;
}

// ---------------------------------------------------------------------------
// Wake word (local spotting).
// ---------------------------------------------------------------------------

let wakeOff: (() => void) | null = null;
let spotter: WakeSpotter | null = null;
let wakeKey = "";
/** Generation of the latest sync: a start that resolves after a newer sync is superseded. */
let wakeGen = 0;
/** The start in flight (opening the mic), shared by syncs with the same key. */
let wakeStarting: { key: string; promise: Promise<void> } | null = null;

/** Whether the wake listener should run with the current config and performance mode. */
export function wakeWanted(): { run: boolean; status: "off" | "needsEnrollment" | "pausedBattery" | "ok" } {
  const c = useVoiceStore.getState().config;
  if (!c?.enabled || !c.wake.enabled) return { run: false, status: "off" };
  const stored = loadWakeTemplates();
  if (!stored || stored.samples.length < 3) return { run: false, status: "needsEnrollment" };
  // The effective mode counts: auto Battery Saver on battery switches it without changing the user's choice.
  const inBatterySaver = usePerfStore.getState().mode === "BatterySaver" || useSettingsStore.getState().settings?.performanceMode === "BatterySaver";
  if (c.wake.pauseOnBatterySaver && inBatterySaver) return { run: false, status: "pausedBattery" };
  return { run: true, status: "ok" };
}

/** (Re)starts or stops the wake listener to match the config. Idempotent. */
export async function syncWakeListener(): Promise<void> {
  const want = wakeWanted();
  const c = useVoiceStore.getState().config;
  const key = want.run && c ? `${c.wake.sensitivity}|${c.wake.cpuBudgetPct}|${c.inputDeviceId ?? ""}|${loadWakeTemplates()?.samples.length ?? 0}` : "";
  // Same target as the start in flight: share it (one segment subscription only).
  if (wakeStarting && wakeStarting.key === key && key !== "") return wakeStarting.promise;
  if (key === wakeKey && !wakeStarting && (want.run ? wakeOff !== null : wakeOff === null)) return;
  stopWake();
  const gen = ++wakeGen;
  wakeKey = key;
  wakeStarting = null;
  const s = useVoiceStore.getState();
  if (!want.run || !c) {
    s.setRuntime({ wakeStatus: want.status === "ok" ? "off" : want.status });
    return;
  }
  const stored = loadWakeTemplates();
  if (!stored) return;
  spotter = new WakeSpotter(stored.samples, c.wake.sensitivity);
  s.setRuntime({ wakeStatus: "starting", wakeError: null });
  const promise = startWake(gen, c.wake.cpuBudgetPct);
  wakeStarting = { key, promise };
  return promise;
}

/**
 * Opens the mic for wake spotting. A start superseded while the mic was
 * opening (wake word off, Battery Saver, device change, shutdown) gives its
 * "wake" claim back unless a newer start owns it — otherwise the microphone
 * stayed open with no consumer.
 */
async function startWake(gen: number, cpuBudgetPct: number): Promise<void> {
  const s = useVoiceStore.getState();
  const current = () => gen === wakeGen;
  /** A newer sync either holds the listener or is opening it: the "wake" claim is theirs. */
  const newerOwnsMic = () => wakeOff !== null || wakeStarting !== null;
  try {
    const m = await openMic("wake");
    if (!current()) {
      if (!newerOwnsMic()) releaseMic("wake");
      return;
    }
    m.setBudget(cpuBudgetPct);
    m.setSegmenting(true);
    const unsub = m.subscribe((msg) => {
      if (msg.type !== "segment" || !spotter) return;
      const st = useVoiceStore.getState();
      // While a command is captured/recognized, or during enrollment, segments are not wake candidates.
      // While the assistant works (thinking), the wake word interrupts it (barge-in) and starts a new command.
      if (st.phase === "listening" || st.phase === "transcribing" || st.mic.includes("enroll")) return;
      if (st.phase === "thinking" && !bargeEnabled()) return;
      const hit = spotter.detect(segmentFrames(msg), Date.now(), msg.early);
      if (hit) onWake();
    });
    const releaseKeepAlive = holdKeepAlive();
    wakeOff = () => {
      unsub();
      releaseKeepAlive();
      releaseMic("wake");
    };
    s.setRuntime({ wakeStatus: "listening" });
  } catch (e) {
    if (!current()) {
      if (!newerOwnsMic()) releaseMic("wake");
      return;
    }
    releaseMic("wake");
    s.setRuntime({ wakeStatus: "error", wakeError: micErrorText(e) });
  } finally {
    if (current()) wakeStarting = null;
  }
}

/**
 * Holds a Web Lock while the wake listener runs: Chromium never freezes a
 * page that holds one, so the listener keeps working while the window is
 * minimized or hidden to the tray (with the WebView2 throttling switches of
 * `webview_policy.rs`). Returns the release function.
 */
function holdKeepAlive(): () => void {
  let release: (() => void) | null = null;
  try {
    const locks = (navigator as Navigator & { locks?: LockManager }).locks;
    void locks?.request("orkestr.voice.wake", () => new Promise<void>((resolve) => (release = resolve))).catch(() => undefined);
  } catch {
    /* no Web Locks: nothing to hold */
  }
  return () => release?.();
}

function stopWake(): void {
  wakeOff?.();
  wakeOff = null;
  spotter = null;
}

function onWake(): void {
  playSound("wake");
  void startListening("WakeWord");
}

/** Stops everything (core disconnect / unmount). */
export function shutdownVoice(): void {
  cancelVoice();
  stopBarge();
  stopWake();
  wakeKey = "";
  // Supersede a start still opening the mic: it releases its claim when it lands.
  wakeGen += 1;
  wakeStarting = null;
  managerWatch?.();
  closeMic();
  useVoiceStore.getState().setRuntime({ wakeStatus: "off" });
}

/**
 * Keeps the barge-in spotter in sync with the voice state (speaking /
 * thinking) and cursor control. Returns the unsubscribe function.
 */
export function installBargeWatcher(): () => void {
  const sync = () => void syncBarge();
  const offVoice = useVoiceStore.subscribe((st, prev) => {
    if (st.speaking !== prev.speaking || st.phase !== prev.phase || st.config !== prev.config || st.mic !== prev.mic) sync();
  });
  const offCursor = useCursorStore.subscribe((st, prev) => {
    if (st.active !== prev.active || st.status !== prev.status) sync();
  });
  sync();
  return () => {
    offVoice();
    offCursor();
    stopBarge();
  };
}
