/**
 * Cursor capture mode (ADR-0050) client state: status from
 * `computer.cursorStatus`, live updates from `ComputerCursor` events (the
 * spoken Jarvis phrases ride on them), screenshots fetched on demand, and
 * the one-time privacy consent flow.
 *
 * Hook for the Jarvis voice UI (wave 9 package A): `useCursorStore(s =>
 * s.active)` is true while the AI controls the cursor — render the orb in
 * mode "cursor" then; `onCursorPhrase` receives every phrase before it is spoken.
 */
import type {
  CursorAction,
  CursorBackendInfo,
  CursorSession,
  CursorSettings,
  CursorShotResult,
  CursorStartResult,
  CursorStatus,
  Event as OkEvent,
} from "@orkestr/protocol";
import { TOPIC_GLOBAL } from "@orkestr/protocol";
import { create } from "zustand";

import { getClient, maybeClient } from "../../core/client";
import { currentLanguage, t } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { useAssistantStore } from "../assistant/store";

export type CursorSource = "voice" | "ui" | "palette" | "orb";

interface CursorStore {
  status: CursorStatus | null;
  /** A session is running (not final). */
  active: boolean;
  /** Screenshot cache (`data:` URLs by shot id). */
  shots: Record<string, string>;
  /** Backend waiting for the privacy consent, with the task to start after it. */
  consent: { backend: CursorBackendInfo; task: string; source: CursorSource } | null;
  /** Voice said "take control" without a task: the next utterance is the task. */
  awaitingTask: boolean;
  error: string | null;
  busy: boolean;
  /** Panel expanded in the assistant surface. */
  open: boolean;
  /** A finished session the user closed. */
  dismissed: string | null;
  patch: (p: Partial<Omit<CursorStore, "patch">>) => void;
}

const FINAL = new Set(["Done", "Aborted", "Failed"]);

export function isActive(s: CursorSession | undefined | null): boolean {
  return !!s && !FINAL.has(s.state);
}

export const useCursorStore = create<CursorStore>((set) => ({
  status: null,
  active: false,
  shots: {},
  consent: null,
  awaitingTask: false,
  error: null,
  busy: false,
  open: false,
  dismissed: null,
  patch: (p) => set(p),
}));

function setStatus(status: CursorStatus): CursorStatus {
  useCursorStore.getState().patch({ status, active: isActive(status.session) });
  return status;
}

/** Spoken phrases: replaced by the Jarvis voice (package A/B) via `setCursorSpeaker`. */
type Speaker = (text: string) => void;
let speaker: Speaker | null = null;
/** Installs the function that speaks cursor-mode phrases (the voice controller's `say`). */
export function setCursorSpeaker(fn: Speaker | null): void {
  speaker = fn;
}
/** Listeners of every phrase (Jarvis voice UI). */
const phraseListeners = new Set<(text: string) => void>();
export function onCursorPhrase(fn: (text: string) => void): () => void {
  phraseListeners.add(fn);
  return () => phraseListeners.delete(fn);
}
function speak(text: string | undefined): void {
  if (!text) return;
  for (const l of phraseListeners) l(text);
  speaker?.(text);
}

export async function refreshCursor(): Promise<CursorStatus | null> {
  const c = maybeClient();
  if (!c) return null;
  try {
    return setStatus(await c.call("computer.cursorStatus", {}));
  } catch (e) {
    useCursorStore.getState().patch({ error: describeError(e) });
    return null;
  }
}

let unsub: (() => void) | null = null;
let refreshTimer: ReturnType<typeof setTimeout> | null = null;

/** Applies one event (exported for tests). */
export function onCursorEvent(ev: OkEvent): void {
  if (ev.kind !== "ComputerCursor") return;
  const p = ev.payload;
  speak(p.say);
  const s = useCursorStore.getState();
  // optimistic state so the badge reacts at once; the full session follows
  if (s.status?.session?.sessionId === p.sessionId) {
    s.patch({
      status: { ...s.status, session: { ...s.status.session, state: p.state } },
      active: !FINAL.has(p.state),
    });
  }
  if (refreshTimer) clearTimeout(refreshTimer);
  refreshTimer = setTimeout(() => {
    refreshTimer = null;
    void refreshCursor();
  }, p.event === "finished" ? 0 : 120);
}

/** Subscribes to `ComputerCursor` events (idempotent). */
export function watchCursor(): void {
  if (unsub) return;
  const c = maybeClient();
  if (!c) return;
  unsub = c.subscribe([TOPIC_GLOBAL], (ev) => onCursorEvent(ev));
}

function handleStart(r: CursorStartResult, task: string, source: CursorSource): CursorStartResult {
  const st = useCursorStore.getState();
  switch (r.status) {
    case "Started":
      st.patch({ consent: null, error: null, open: true, active: true, status: st.status ? { ...st.status, session: r.session } : st.status });
      void refreshCursor();
      break;
    case "NeedsConsent":
      st.patch({ consent: { backend: r.backend, task, source }, open: true });
      break;
    case "NoBackend": {
      const s = useCursorStore.getState().status;
      const reason = s
        ? s.backends
            .filter((b) => b.source === s.settings.source && b.id !== "mock")
            .map((b) => `${b.label}: ${detailText(b.detail)}`)
            .join("; ")
        : r.reason;
      st.patch({ error: t("cursor:errors.noBackend", { reason }), open: true });
      speak(t("cursor:say.noBackend"));
      break;
    }
  }
  return r;
}

/** Opens Assistant mode with the cursor panel expanded. */
export function openCursorPanel(): void {
  useAssistantStore.getState().setMode("assistant");
  useCursorStore.getState().patch({ open: true });
  watchCursor();
}

/** Starts a session (asks for consent first when needed). */
export async function startCursor(task: string, source: CursorSource): Promise<CursorStartResult | null> {
  const text = task.trim();
  if (!text) return null;
  const st = useCursorStore.getState();
  st.patch({ busy: true, error: null, awaitingTask: false });
  watchCursor();
  try {
    const r = await getClient().call("computer.cursorStart", { task: text, lang: currentLanguage(), source });
    return handleStart(r, text, source);
  } catch (e) {
    st.patch({ error: describeError(e), open: true });
    return null;
  } finally {
    useCursorStore.getState().patch({ busy: false });
  }
}

/** Grants the pending consent and starts the task. */
export async function grantConsentAndStart(): Promise<void> {
  const c = useCursorStore.getState().consent;
  if (!c) return;
  await getClient().call("computer.cursorConsent", { backend: c.backend.id, granted: true });
  useCursorStore.getState().patch({ consent: null });
  await startCursor(c.task, c.source);
}

export function declineConsent(): void {
  useCursorStore.getState().patch({ consent: null });
}

async function control(method: "computer.cursorStop" | "computer.cursorPause" | "computer.cursorResume"): Promise<void> {
  try {
    setStatus(await getClient().call(method, {}));
  } catch (e) {
    useCursorStore.getState().patch({ error: describeError(e) });
  }
}

export const stopCursor = () => control("computer.cursorStop");
export const pauseCursor = () => control("computer.cursorPause");
export const resumeCursor = () => control("computer.cursorResume");

export async function answerCursor(text: string): Promise<void> {
  try {
    setStatus(await getClient().call("computer.cursorAnswer", { text }));
  } catch (e) {
    useCursorStore.getState().patch({ error: describeError(e) });
  }
}

export async function confirmCursor(approve: boolean): Promise<void> {
  try {
    setStatus(await getClient().call("computer.cursorConfirm", { approve }));
  } catch (e) {
    useCursorStore.getState().patch({ error: describeError(e) });
  }
}

export async function saveCursorSettings(settings: CursorSettings): Promise<void> {
  try {
    setStatus(await getClient().call("computer.cursorSetSettings", { settings }));
  } catch (e) {
    useCursorStore.getState().patch({ error: describeError(e) });
  }
}

export async function setConsent(backend: string, granted: boolean): Promise<void> {
  try {
    setStatus(await getClient().call("computer.cursorConsent", { backend, granted }));
  } catch (e) {
    useCursorStore.getState().patch({ error: describeError(e) });
  }
}

/** Loads (and caches) a screenshot as a `data:` URL. */
export async function loadShot(id: string): Promise<string | null> {
  const cached = useCursorStore.getState().shots[id];
  if (cached) return cached;
  try {
    const r: CursorShotResult = await getClient().call("computer.cursorScreenshot", { id });
    const url = `data:${r.mediaType};base64,${r.dataBase64}`;
    const shots = { ...useCursorStore.getState().shots };
    const keys = Object.keys(shots);
    if (keys.length > 12) delete shots[keys[0]!];
    shots[id] = url;
    useCursorStore.getState().patch({ shots });
    return url;
  } catch {
    return null;
  }
}

/** Localized backend detail (`notFound`, `subscription:max`, `key:OpenAI`, …). */
export function detailText(detail: string): string {
  const i = detail.indexOf(":");
  const code = i < 0 ? detail : detail.slice(0, i);
  const value = i < 0 ? "" : detail.slice(i + 1);
  return t(`cursor:detail.${code}`, { value, defaultValue: detail });
}

/** One-line localized description of an action. */
export function actionText(a: CursorAction): string {
  const target = "element" in a && a.element != null ? `#${a.element}` : "x" in a && a.x != null ? `${a.x},${a.y ?? 0}` : "";
  switch (a.type) {
    case "click":
      return t(a.double ? "cursor:action.doubleClick" : a.button === "Right" ? "cursor:action.rightClick" : "cursor:action.click", { target });
    case "move":
      return t("cursor:action.move", { target });
    case "drag":
      return t("cursor:action.drag");
    case "scroll":
      return t("cursor:action.scroll", { direction: t(`cursor:direction.${a.direction}`), amount: a.amount });
    case "type":
      return t("cursor:action.type", { n: [...a.text].length });
    case "keys":
      return t("cursor:action.keys", { keys: a.keys.join("+") });
    case "wait":
      return t("cursor:action.wait", { ms: a.ms });
    case "focus_window":
      return t("cursor:action.focus", { title: a.title });
    case "launch":
      return t("cursor:action.launch", { app: a.app });
    case "ask_user":
      return t("cursor:action.ask");
    case "done":
      return t("cursor:action.done");
  }
}
