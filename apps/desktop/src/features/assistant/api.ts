/**
 * Assistant actions over `computer.*` (ADR-0035): run an utterance, confirm
 * a risky run, review / correct / approve / remember an AI plan, global
 * stop. Every result updates the assistant store, applies client effects
 * (mode switch, spoken text, lists) and yields a short localized reply.
 */
import type { ClientEffect, ComputerAction, HandleResult, RiskReason } from "@orkestr/protocol";

import { getClient, maybeClient } from "../../core/client";
import { currentLanguage, t } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { useVoiceStore } from "../voice/store";
import { naturalRunReply, type PhraseOptions } from "./phrasing";
import { type AssistantReply, type AssistantTone, useAssistantStore } from "./store";

export type AssistantOutcome = "ok" | "failed" | "needsConfirmation" | "proposed" | "cancelled" | "notUnderstood";

export interface AssistantRun {
  outcome: AssistantOutcome;
  reply: string;
  tone: AssistantTone;
  /** Text to speak in addition to the reply (`Speak` steps). */
  speak: string[];
  /** The reply was already spoken (streamed Jarvis conversation) or stays silent. */
  spoken?: boolean;
}

/** Localized risk reason. */
export function riskText(r: RiskReason): string {
  return t(`assistant:risk.${r.code}`, { detail: r.detail, defaultValue: `${r.code}: ${r.detail}` });
}

/** One-line description of a step. */
export function stepText(s: ComputerAction): string {
  const fields: Record<string, unknown> = { ...s };
  delete fields["type"];
  const values: Record<string, string> = { app: "", engine: "", value: "", amount: "", message: "" };
  for (const [k, v] of Object.entries(fields)) values[k] = typeof v === "object" ? JSON.stringify(v) : String(v ?? "");
  if (s.type === "Window") values["op"] = t(`assistant:windowOp.${s.op}`);
  const key = s.type === "CloseApp" && s.discard ? "CloseAppDiscard" : s.type;
  return t(`assistant:step.${key}`, { ...values, defaultValue: s.type }).replace(/\s+/g, " ").trim();
}

function applyEffects(effects: ClientEffect[]): string[] {
  const speak: string[] = [];
  const store = useAssistantStore.getState();
  for (const e of effects) {
    switch (e.type) {
      case "SetMode":
        store.setMode(e.mode === "Assistant" ? "assistant" : "orchestra");
        break;
      case "Speak":
        speak.push(e.text);
        break;
      case "Show":
        store.patch({ shown: { title: t(`assistant:shown.${e.title}`, { defaultValue: e.title }), lines: e.lines } });
        break;
      case "Screenshot":
        store.patch({ shown: { title: t("assistant:shown.screenshot"), lines: [e.path] } });
        break;
    }
  }
  return speak;
}

/** Turns a core result into store state + a reply. */
export function applyResult(r: HandleResult, phrase: PhraseOptions = {}): AssistantRun {
  const store = useAssistantStore.getState();
  const out = (outcome: AssistantOutcome, reply: string, tone: AssistantTone, speak: string[] = []): AssistantRun => {
    store.patch({ reply: { text: reply, tone } });
    return { outcome, reply, tone, speak };
  };
  switch (r.status) {
    case "Executed": {
      store.patch({ pending: null });
      const speak = applyEffects(r.run.effects);
      switch (r.run.outcome) {
        case "Succeeded":
          // natural speech ("Opening Calculator"), not a status line
          return out("ok", naturalRunReply(r.run, phrase), "ok", speak);
        case "Stopped":
          return out("cancelled", t("assistant:reply.stopped"), "warn");
        case "Rejected":
          return out("cancelled", t("assistant:reply.rejected"), "info");
        case "Failed":
          return out("failed", naturalRunReply(r.run, phrase), "error");
      }
      break;
    }
    case "NeedsConfirmation":
      store.patch({ pending: r.pending, plan: null });
      return out("needsConfirmation", t("assistant:reply.confirm", { title: r.pending.title, reasons: r.pending.reasons.map(riskText).join("; ") }), "warn");
    case "Proposed":
      store.patch({ plan: r.plan, pending: null, ranPlanId: null });
      return out("proposed", t("assistant:reply.proposed", { explanation: r.plan.explanation }), "info");
    case "Remembered":
      store.patch({ plan: null, ranPlanId: null });
      return out("ok", t("assistant:reply.remembered", { phrase: r.command.phrases[0] ?? r.command.id }), "ok");
    case "NotUnderstood":
      return r.reason
        ? out("notUnderstood", t("assistant:reply.notUnderstoodReason", { reason: r.reason }), "warn")
        : out("notUnderstood", t("assistant:reply.notUnderstood"), "warn");
  }
  return out("failed", t("assistant:reply.notUnderstood"), "warn");
}

function lang(): string {
  return currentLanguage();
}

async function guarded(input: string | null, fn: () => Promise<HandleResult>, phrase: PhraseOptions = {}): Promise<AssistantRun> {
  const store = useAssistantStore.getState();
  store.patch({ busy: true, ...(input !== null ? { input, shown: null } : {}) });
  try {
    return applyResult(await fn(), phrase);
  } catch (e) {
    const reply: AssistantReply = { text: t("assistant:reply.error", { error: describeError(e) }), tone: "error" };
    store.patch({ reply });
    return { outcome: "failed", reply: reply.text, tone: "error", speak: [] };
  } finally {
    useAssistantStore.getState().patch({ busy: false });
    void refreshHistory();
  }
}

/** Runs an utterance (matcher → command, or an AI plan when `planUnknown`). */
export function runAssistant(text: string, source: "voice" | "text", planUnknown = true): Promise<AssistantRun> {
  return guarded(text, () => getClient().call("computer.handle", { text, lang: lang(), planUnknown, source }));
}

/** Runs a catalogue / user command by id (quick search). */
export function runCommand(commandId: string, setId: string, slots: Record<string, string>, title: string): Promise<AssistantRun> {
  return guarded(title, () => getClient().call("computer.run", { commandId, setId, slots, lang: lang() }));
}

/** Confirms / rejects the pending risky run. */
export function confirmPending(approve: boolean): Promise<AssistantRun> {
  const p = useAssistantStore.getState().pending;
  if (!p) return Promise.resolve({ outcome: "cancelled", reply: "", tone: "info", speak: [] });
  return guarded(null, () => getClient().call("computer.confirm", { pendingId: p.pendingId, approve }));
}

/** Asks the AI for a corrected plan. */
export async function correctPlan(correction: string): Promise<AssistantRun> {
  const plan = useAssistantStore.getState().plan;
  if (!plan) return { outcome: "cancelled", reply: "", tone: "info", speak: [] };
  return guarded(null, async () => {
    const r = await getClient().call("computer.plan", { text: "", planId: plan.planId, correction, lang: lang() });
    return { status: "Proposed", plan: r.plan } satisfies HandleResult;
  });
}

/** Runs the plan under review. */
export async function approvePlan(): Promise<AssistantRun> {
  const plan = useAssistantStore.getState().plan;
  if (!plan) return { outcome: "cancelled", reply: "", tone: "info", speak: [] };
  const r = await guarded(null, () => getClient().call("computer.approvePlan", { planId: plan.planId }), { planned: true });
  if (r.outcome === "ok") useAssistantStore.getState().patch({ ranPlanId: plan.planId });
  return phrasePlanReply(r, plan.request);
}

/** AI phrasing budget for plan results (same limit as the voice replies). */
export const PLAN_PHRASE_TIMEOUT_MS = 5000;

/**
 * AI-planned runs (long, several steps): when natural replies are on, the
 * Manager's model may phrase the result (`voice.phrase`, 5 s limit); the
 * instant template reply stays on any failure / timeout. Single commands
 * never wait for this.
 */
async function phrasePlanReply(r: AssistantRun, request: string): Promise<AssistantRun> {
  if ((r.outcome !== "ok" && r.outcome !== "failed") || !r.reply) return r;
  if (useVoiceStore.getState().config?.naturalReplies !== true) return r;
  try {
    const facts = `The user asked the computer assistant: "${request}". Outcome: ${r.outcome === "ok" ? "done" : "failed"}. Result: ${r.reply}`;
    const res = await getClient().call("voice.phrase", { facts, lang: lang(), timeoutMs: PLAN_PHRASE_TIMEOUT_MS }, { timeoutMs: PLAN_PHRASE_TIMEOUT_MS + 500 });
    const text = res.text.trim();
    if (!text) return r;
    useAssistantStore.getState().patch({ reply: { text, tone: r.tone } });
    return { ...r, reply: text };
  } catch {
    return r;
  }
}

/** Dismisses the plan under review. */
export function dismissPlan(): void {
  useAssistantStore.getState().patch({ plan: null, ranPlanId: null, reply: { text: t("assistant:reply.planDismissed"), tone: "info" } });
}

/** Saves the plan as a new user command. */
export async function rememberPlan(phrase: string): Promise<AssistantRun> {
  const plan = useAssistantStore.getState().plan;
  if (!plan) return { outcome: "cancelled", reply: "", tone: "info", speak: [] };
  return guarded(null, async () => {
    const r = await getClient().call("computer.rememberPlan", { planId: plan.planId, phrases: phrase.trim() ? [phrase.trim()] : [] });
    return { status: "Remembered", setId: r.setId, command: r.command } satisfies HandleResult;
  });
}

/** Global stop of the assistant (runs, confirmations, timers). */
export async function stopAssistant(): Promise<void> {
  try {
    const r = await getClient().call("computer.stop", {});
    useAssistantStore.getState().patch({
      pending: null,
      busy: false,
      reply: { text: t("assistant:reply.stoppedAll", { count: r.runsStopped + r.pendingCancelled + r.timersCancelled }), tone: "warn" },
    });
  } catch (e) {
    useAssistantStore.getState().patch({ reply: { text: describeError(e), tone: "error" } });
  }
}

/** Recent commands + status (best effort). */
export async function refreshHistory(): Promise<void> {
  const client = maybeClient();
  if (!client) return;
  try {
    const [h, status] = await Promise.all([client.call("computer.history", { limit: 8 }), client.call("computer.status", {})]);
    useAssistantStore.getState().patch({ history: h.entries, status });
  } catch {
    /* core without the assistant (older daemon): keep the panel usable */
  }
}
