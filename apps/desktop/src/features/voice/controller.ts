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
 */
import type { Event, VoiceCommandSource } from "@orkestr/protocol";
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
import { routeUtterance } from "../assistant/voiceRoute";
import { dailyGreeting } from "../jarvis/greeting";
import { isBusy, type MachineEvent, nextPhase } from "./pipeline/machine";
import { speakable } from "./pipeline/summary";
import { type MicPurpose, useVoiceStore } from "./store";
import { engineNeedsPcm, SttError, type SttSession, startStt, webSpeechAvailable } from "./stt/engines";
import { speak, speechLang, stopSpeaking } from "./tts/speaker";
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

/** Push-to-talk pressed / HUD mic clicked / wake word heard. */
export async function startListening(source: VoiceCommandSource): Promise<void> {
  const s = useVoiceStore.getState();
  const c = s.config;
  if (!c?.enabled) return;
  if (isBusy(s.phase) || session) return;
  clearDismiss();
  stopSpeaking();
  if (!sttReady()) {
    // No recognition: open the HUD for a typed command.
    s.setHudOpen(true);
    s.setRuntime({ source: "Typed" });
    // Say why there is no listening (instead of a generic "unavailable").
    if (c.sttEngine === "Disabled" && source !== "Typed") s.setRuntime({ reply: { text: t("voice:errors.sttDisabled"), tone: "info", outcome: "ok" } });
    return;
  }
  if (!emit({ type: "listen" })) return;
  s.setRuntime({ source, transcript: "", reply: pending ? s.reply : null });
  if (source !== "WakeWord") playSound("listenStart");
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
    session = startStt({ engine: c.sttEngine, lang: lang(), mic: m, autoEnd: source === "WakeWord" });
    sessionSource = source;
    session.onInterim((text) => useVoiceStore.getState().setRuntime({ transcript: text }));
    session.onAutoEnd(() => void stopListening());
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
    fail(e instanceof SttError ? sttErrorText(e) : describeError(e));
    return;
  }
  releaseMic("command");
  if (!text) {
    fail(t("voice:errors.noSpeech"));
    return;
  }
  useVoiceStore.getState().setRuntime({ transcript: text });
  emit({ type: "transcript", text });
  await handleText(text, source);
}

/** Cancels whatever the pipeline is doing (Esc, HUD close). */
export function cancelVoice(): void {
  startGen += 1;
  starting = false;
  session?.cancel();
  session = null;
  pending = null;
  releaseMic("command");
  stopSpeaking();
  clearDismiss();
  emit({ type: "cancel" });
  useVoiceStore.getState().setRuntime({ confirmText: null, transcript: "", reply: null });
}

/** A typed command from the HUD. */
export async function submitTyped(text: string): Promise<void> {
  const clean = text.trim();
  if (!clean) return;
  clearDismiss();
  stopSpeaking();
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
  // Computer assistant (ADR-0035): computer commands, its confirmations and AI plans.
  // Jarvis (ADR-0049): AI conversation replies stream into the HUD and are spoken sentence by sentence.
  const viaAssistant = await routeUtterance(text, intent, wakePhrases(), {
    speak: (sentence) => say(sentence),
    onPartial: (partial) => {
      if (partial) useVoiceStore.getState().setRuntime({ reply: { text: partial, tone: "info", outcome: "ok" } });
    },
  }).catch(() => null);
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
    void say(`${prompt} ${t("voice:confirm.sayYesNo")}`);
    void record(text, intent, "needsConfirmation", source, null);
    return;
  }
  await run(intent, source);
}

async function run(intent: Intent, source: VoiceCommandSource): Promise<void> {
  let r: CommandResult;
  try {
    r = await executeIntent(intent, getDeps());
  } catch (e) {
    r = { outcome: "failed", reply: t("voice:reply.failed", { error: describeError(e) }), tone: "error", projectId: null };
  }
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
  if (!c?.spokenReplies || !opts) return;
  useVoiceStore.getState().setRuntime({ speaking: true });
  try {
    await speak(speakable(text, 260), opts);
  } finally {
    useVoiceStore.getState().setRuntime({ speaking: false });
  }
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
      // While a command is captured/processed, or during enrollment, segments are not wake candidates.
      if (st.phase === "listening" || isBusy(st.phase) || st.mic.includes("enroll")) return;
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
  stopWake();
  wakeKey = "";
  // Supersede a start still opening the mic: it releases its claim when it lands.
  wakeGen += 1;
  wakeStarting = null;
  managerWatch?.();
  closeMic();
  useVoiceStore.getState().setRuntime({ wakeStatus: "off" });
}
