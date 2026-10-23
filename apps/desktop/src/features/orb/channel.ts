/**
 * Main window ↔ orb window signals that are not business data (ADR-0045):
 * the voice state the sphere visualizes (phase, speaking, input level),
 * whether an Orkestr window has the focus (decorative motion pauses
 * otherwise), and push-to-talk from the hub's mic button. Same-origin
 * `BroadcastChannel`, plain data only (never audio).
 */
import { useEffect, useRef, useState } from "react";

const CHANNEL = "orkestr.orb";

export interface OrbVoiceState {
  /** Voice is configured and enabled. */
  enabled: boolean;
  /** Voice phase (`idle`, `listening`, `transcribing`, `thinking`, …). */
  phase: string;
  /** The assistant is speaking. */
  speaking: boolean;
  /** Microphone input level 0..1. */
  level: number;
  /** The microphone is open (privacy indicator). */
  micOpen?: boolean;
  /** What was heard (voice popup next to the orb). */
  transcript?: string;
  /** The reply shown / spoken. */
  reply?: { text: string; tone: string } | null;
  /** A pending yes/no question. */
  confirmText?: string | null;
}

/** Orb popup → main window: answers / closes the voice command. */
export type OrbVoiceAction = "yes" | "no" | "cancel";

export interface OrbMainState {
  voice: OrbVoiceState;
  /** The main window (or another Orkestr window) has the focus. */
  appActive: boolean;
}

export type OrbMessage =
  | { type: "state"; state: OrbMainState }
  | { type: "hello" }
  | { type: "ptt"; down: boolean }
  | { type: "voice"; action: OrbVoiceAction };

export const IDLE_MAIN_STATE: OrbMainState = {
  voice: { enabled: false, phase: "idle", speaking: false, level: 0 },
  appActive: true,
};

export function openChannel(): BroadcastChannel | null {
  try {
    return typeof BroadcastChannel === "function" ? new BroadcastChannel(CHANNEL) : null;
  } catch {
    return null;
  }
}

/** Posts unless the channel is already closed; never throws. */
export function safePost(ch: BroadcastChannel | null, msg: OrbMessage): void {
  try {
    ch?.postMessage(msg);
  } catch {
    // Closed channel (window tearing down).
  }
}

/** Whether a message is a well-formed state message. */
export function isStateMessage(m: unknown): m is { type: "state"; state: OrbMainState } {
  if (!m || typeof m !== "object") return false;
  const msg = m as { type?: unknown; state?: { voice?: unknown; appActive?: unknown } };
  return msg.type === "state" && typeof msg.state?.appActive === "boolean" && typeof msg.state.voice === "object" && msg.state.voice !== null;
}

/**
 * Orb window: the main window's state, live (`IDLE_MAIN_STATE` until it
 * answers), and a push-to-talk sender.
 */
export function useOrbMainState(): { state: OrbMainState; ptt: (down: boolean) => void; voiceAction: (action: OrbVoiceAction) => void } {
  const [state, setState] = useState<OrbMainState>(IDLE_MAIN_STATE);
  const chRef = useRef<BroadcastChannel | null>(null);
  useEffect(() => {
    const ch = openChannel();
    if (!ch) return undefined;
    chRef.current = ch;
    ch.onmessage = (e: MessageEvent<unknown>) => {
      if (isStateMessage(e.data)) setState(e.data.state);
    };
    safePost(ch, { type: "hello" });
    return () => {
      if (chRef.current === ch) chRef.current = null;
      ch.onmessage = null;
      ch.close();
    };
  }, []);
  const ptt = (down: boolean) => safePost(chRef.current, { type: "ptt", down });
  const voiceAction = (action: OrbVoiceAction) => safePost(chRef.current, { type: "voice", action });
  return { state, ptt, voiceAction };
}

/**
 * Whether the voice popup should be open next to the orb: a voice command is
 * in progress while no Orkestr window has the focus (main window minimized,
 * hidden to the tray or in the background). Once open it stays open until
 * the command finishes (phase back to idle).
 */
export function voicePopupWanted(voice: OrbVoiceState, appActive: boolean, open: boolean): boolean {
  if (voice.phase === "idle") return false;
  return open || !appActive;
}
