/**
 * Voice state across windows (§33, §34): the voice engine runs in the main
 * window; the HUD window shows its microphone state and can toggle the
 * voice HUD. Same-origin `BroadcastChannel`, plain data only (never audio).
 * The privacy rule holds in the HUD too: while the mic is open it says so.
 */
import { useCallback, useEffect, useRef, useState } from "react";

import { cancelVoice } from "../controller";
import { type MicPurpose, useVoiceStore } from "../store";

const CHANNEL = "orkestr.voice";

export interface VoiceSnapshot {
  /** Voice is configured and enabled. */
  enabled: boolean;
  /** Why the microphone is open (empty = closed). */
  mic: MicPurpose[];
  /** Listening / thinking / speaking … (`idle` when nothing runs). */
  phase: string;
  /** Input level 0..1 (rounded). */
  level: number;
}

type Message = { type: "state"; state: VoiceSnapshot } | { type: "hello" } | { type: "toggle" };

function channel(): BroadcastChannel | null {
  try {
    return typeof BroadcastChannel === "function" ? new BroadcastChannel(CHANNEL) : null;
  } catch {
    return null;
  }
}

function snapshot(): VoiceSnapshot {
  const s = useVoiceStore.getState();
  return { enabled: !!s.config?.enabled, mic: [...s.mic], phase: s.phase, level: Math.round(s.level * 10) / 10 };
}

/** Same toggle as the title-bar mic button. */
function toggleVoiceHud(): void {
  const s = useVoiceStore.getState();
  if (s.hudOpen || s.phase !== "idle") {
    cancelVoice();
    s.setHudOpen(false);
  } else s.setHudOpen(true);
}

/** Main window: publishes the voice state and answers HUD requests. Returns a disposer. */
export function startVoiceBroadcast(): () => void {
  const ch = channel();
  if (!ch) return () => undefined;
  let last = "";
  const publish = () => {
    const state = snapshot();
    const key = JSON.stringify(state);
    if (key === last) return;
    last = key;
    ch.postMessage({ type: "state", state } satisfies Message);
  };
  const off = useVoiceStore.subscribe(publish);
  ch.onmessage = (e: MessageEvent<Message>) => {
    if (e.data?.type === "hello") {
      last = "";
      publish();
    } else if (e.data?.type === "toggle") toggleVoiceHud();
  };
  publish();
  return () => {
    off();
    ch.close();
  };
}

/** Other windows (HUD): the main window's voice state, live; `null` until it answers. */
export function useRemoteVoice(): { state: VoiceSnapshot | null; toggle: () => void } {
  const [state, setState] = useState<VoiceSnapshot | null>(null);
  // The channel belongs to one effect run: created there and closed in that
  // run's cleanup. (A channel created once per component and closed in the
  // cleanup threw "Channel is closed" when StrictMode / Fast Refresh re-ran
  // the effect, which unmounted the whole HUD.)
  const chRef = useRef<BroadcastChannel | null>(null);
  useEffect(() => {
    const ch = channel();
    if (!ch) return undefined;
    chRef.current = ch;
    ch.onmessage = (e: MessageEvent<Message>) => {
      if (e.data?.type === "state") setState(e.data.state);
    };
    safePost(ch, { type: "hello" });
    return () => {
      if (chRef.current === ch) chRef.current = null;
      ch.onmessage = null;
      ch.close();
    };
  }, []);
  const toggle = useCallback(() => {
    const ch = chRef.current;
    if (ch) safePost(ch, { type: "toggle" });
  }, []);
  return { state, toggle };
}

/** Posts unless the channel is already closed; never throws into React. */
function safePost(ch: BroadcastChannel, msg: Message): void {
  try {
    ch.postMessage(msg);
  } catch {
    // Closed channel (window tearing down): nothing to tell.
  }
}
