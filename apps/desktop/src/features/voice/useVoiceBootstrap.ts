/**
 * Wires voice + sound once the core connection is ready: loads the voice
 * config, registers the sound environment, the push-to-talk hold hotkey,
 * voice notifications / sound cues for core events, and keeps the local
 * wake-word listener in sync with config, performance mode and focus.
 */
import type { Event } from "@orkestr/protocol";
import { useEffect } from "react";

import { getClient } from "../../core/client";
import { t } from "../../i18n";
import { useAppStore } from "../../store/app";
import { useSettingsStore } from "../settings/settingsStore";
import { notificationTopics } from "../settings/useShellBootstrap";
import { usePerfStore } from "../performance/perfStore";
import { configureSound, playSound } from "../sound/engine";
import { activeTheme, useThemeStore } from "../theme/themeStore";
import { assistantOptions, robotVoiceFor, speakAsAgent } from "./agentVoices";
import { installVoiceDebugHook, shutdownVoice, startListening, stopListening, syncWakeListener } from "./controller";
import { isHoldEnd, isHoldStart, parseHotkey } from "./hotkey";
import { noticeFromEvent, phraseId, PhraseLimiter, speakDecision } from "./notify/policy";
import { useVoiceStore } from "./store";
import { startVoiceBroadcast } from "./ui/voiceBridge";

/** Events older than this (replays after reconnect) are never announced. */
const MAX_EVENT_AGE_MS = 60_000;

const limiter = new PhraseLimiter();

function announce(ev: Event): void {
  const n = noticeFromEvent(ev);
  if (!n) return;
  if (Date.now() - Date.parse(ev.ts) > MAX_EVENT_AGE_MS) return;
  playSound(n.cue);
  const v = useVoiceStore.getState();
  const c = v.config;
  if (!c) return;
  const decision = speakDecision(n, {
    config: c.notifications,
    voiceEnabled: c.enabled && assistantOptions() !== null,
    doNotDisturb: useSettingsStore.getState().settings?.doNotDisturb ?? false,
    now: new Date(),
  });
  if (decision !== "speak") return;
  // A robot with its voice on speaks for its agent (RobotHost).
  if (robotVoiceFor(n.agentId ?? null)?.enabled) return;
  // Never talk over the user or a reply in progress.
  if (v.phase !== "idle" || v.speaking) return;
  if (!limiter.admit(n.id, Date.now())) return;
  const phrase = t(`voice:notify.phrases.${phraseId(n)}`, { percent: n.vars.percent ?? 0 });
  const project = n.projectId ? useAppStore.getState().projects.find((p) => p.id === n.projectId)?.name : undefined;
  const text = project ? t("voice:notify.withProject", { project, phrase }) : phrase;
  void speakAsAgent(n.agentId, text);
}

export function useVoiceBootstrap(ready: boolean): void {
  // Config + sound environment + wake-listener sync.
  useEffect(() => {
    if (!ready) return;
    configureSound(() => {
      const c = useVoiceStore.getState().config;
      const s = useSettingsStore.getState().settings;
      if (!c) return null;
      return {
        config: c.sound,
        doNotDisturb: s?.doNotDisturb ?? false,
        performanceMode: usePerfStore.getState().mode,
        themeId: useAppStore.getState().themeId,
        themeSounds: activeTheme(useThemeStore.getState()).sounds,
      };
    });
    void useVoiceStore
      .getState()
      .load()
      .then(() => syncWakeListener());
    const offVoice = useVoiceStore.subscribe((s, prev) => {
      if (s.config !== prev.config) void syncWakeListener();
    });
    const offBridge = startVoiceBroadcast();
    const offDebug = installVoiceDebugHook();
    const offSettings = useSettingsStore.subscribe((s, prev) => {
      if (s.settings?.performanceMode !== prev.settings?.performanceMode) void syncWakeListener();
    });
    return () => {
      offVoice();
      offBridge();
      offDebug();
      offSettings();
      configureSound(() => null);
      shutdownVoice();
    };
  }, [ready]);

  // Voice notifications + sound cues for core events.
  useEffect(() => {
    if (!ready) return;
    let key = "";
    let unsubscribe: (() => void) | null = null;
    const resubscribe = () => {
      const ids = useAppStore.getState().projects.map((p) => p.id);
      const next = ids.join(",");
      if (unsubscribe && next === key) return;
      key = next;
      unsubscribe?.();
      unsubscribe = getClient().subscribe(notificationTopics(ids), announce);
    };
    resubscribe();
    const off = useAppStore.subscribe((s, prev) => {
      if (s.projects !== prev.projects) resubscribe();
    });
    return () => {
      off();
      unsubscribe?.();
    };
  }, [ready]);

  // Push-to-talk: hold the hotkey (default Ctrl+Space) while speaking.
  useEffect(() => {
    if (!ready) return;
    let holding = false;
    const hotkey = () => parseHotkey(useVoiceStore.getState().config?.pushToTalkHotkey ?? "Ctrl+Space");
    const onDown = (e: KeyboardEvent) => {
      const h = hotkey();
      const c = useVoiceStore.getState().config;
      if (!h || !c?.enabled || !isHoldStart(e, h)) return;
      e.preventDefault();
      e.stopPropagation();
      if (e.repeat || holding) return;
      holding = true;
      void startListening("PushToTalk");
    };
    const onUp = (e: KeyboardEvent) => {
      const h = hotkey();
      if (!holding || !h || !isHoldEnd(e, h)) return;
      holding = false;
      void stopListening();
    };
    const onBlur = () => {
      if (!holding) return;
      holding = false;
      void stopListening();
    };
    window.addEventListener("keydown", onDown, { capture: true });
    window.addEventListener("keyup", onUp, { capture: true });
    window.addEventListener("blur", onBlur);
    return () => {
      window.removeEventListener("keydown", onDown, { capture: true });
      window.removeEventListener("keyup", onUp, { capture: true });
      window.removeEventListener("blur", onBlur);
    };
  }, [ready]);
}
