/**
 * Main-window side of the orb (ADR-0045): shows / hides / resizes the orb
 * window from the orb preferences, answers orb requests (open the main
 * window, an agent, a Claude Code chat, settings, approvals), publishes the
 * voice state and app focus the sphere visualizes, runs push-to-talk for
 * the hub's mic button, and migrates the old robots switch once.
 */
import { useEffect } from "react";

import { workspaceActions } from "../../components/workspaceActions";
import { maybeClient, openProject } from "../../core/client";
import { t } from "../../i18n";
import { useAgentsUi } from "../../panels/Agents/store";
import { useAppStore } from "../../store/app";
import { useAiUiStore, useUiPrefs } from "../aiui/aiuiStore";
import { useClaudeStore } from "../claudeCode/store";
import { useSecurityStore } from "../security/store";
import { useSettingsStore } from "../settings/settingsStore";
import { answerConfirmation, cancelVoice, startListening, stopListening } from "../voice/controller";
import { useVoiceStore } from "../voice/store";
import { type OrbMainState, isStateMessage, openChannel, safePost } from "./channel";
import { orbBoxSize, orbPrefsOf } from "./model";
import { ORB_EVENTS, orbShell } from "./shell";

/** Local flag: the robots → orb migration ran on this device. */
export const MIGRATED_KEY = "orkestr.orb.migratedFromRobots";

/** The orb setting implied by the old robots document (`null` = keep the default). */
export function migratedEnabled(robots: { enabled?: unknown } | null | undefined): boolean | null {
  return robots && robots.enabled === false ? false : null;
}

async function openAt(projectId: unknown): Promise<void> {
  if (typeof projectId === "string" && projectId && useAppStore.getState().activeProjectId !== projectId) await openProject(projectId);
}

/** Opens the Agents panel with the agent selected. */
export async function focusAgent(agentId: string, projectId: unknown): Promise<void> {
  await openAt(projectId);
  workspaceActions.openPanel("agents");
  const ui = useAgentsUi.getState();
  ui.setTab("agents");
  ui.select(agentId);
}

function mainState(): OrbMainState {
  const v = useVoiceStore.getState();
  return {
    voice: {
      enabled: !!v.config?.enabled,
      phase: v.phase,
      speaking: v.speaking,
      level: Math.round(v.level * 10) / 10,
      micOpen: v.mic.includes("command"),
      transcript: v.transcript,
      reply: v.reply ? { text: v.reply.text, tone: v.reply.tone } : null,
      confirmText: v.confirmText,
    },
    appActive: typeof document !== "undefined" && document.hasFocus() && !document.hidden,
  };
}

export function OrbHost() {
  const connected = useAppStore((s) => s.connection.state === "open");
  const loaded = useAiUiStore((s) => s.config !== null);
  const prefs = orbPrefsOf(useUiPrefs().orb);
  const box = orbBoxSize(prefs.size);
  const ready = connected && loaded;
  // A voice command while Orkestr is in the background shows its popup next
  // to the orb — even with the orb switched off (popup only, no sphere).
  const voiceBusy = useVoiceStore((s) => s.phase !== "idle");
  const backgroundVoice = voiceBusy && !(typeof document !== "undefined" && document.hasFocus() && !document.hidden);
  const show = prefs.enabled || backgroundVoice;

  // Window lifecycle. Nothing happens until the preferences are known: a
  // reloading main window must not close (and re-create) a visible orb.
  useEffect(() => {
    if (!orbShell.available() || !ready) return;
    (show ? orbShell.show(box) : orbShell.hide()).catch((e: unknown) => console.warn("orb window", e));
  }, [ready, show, box]);

  // Requests from the orb window.
  useEffect(() => {
    const subs = [
      orbShell.on<{ agentId?: string; projectId?: string }>(ORB_EVENTS.openAgent, (p) => {
        if (p.agentId) void focusAgent(p.agentId, p.projectId);
      }),
      orbShell.on<{ sessionId?: string; projectId?: string; claudeSessionId?: string | null }>(ORB_EVENTS.openClaude, (p) => {
        if (!p.projectId || !p.sessionId) return;
        void openAt(p.projectId).then(async () => {
          workspaceActions.openPanel("claudeCode");
          await useClaudeStore.getState().openChat(p.projectId!, { sessionId: p.sessionId ?? null, claudeSessionId: p.claudeSessionId ?? null });
        });
      }),
      orbShell.on<{ section?: string }>(ORB_EVENTS.openSettings, (p) => useSettingsStore.getState().openSettings(p.section ?? "orb")),
      orbShell.on(ORB_EVENTS.openApprovals, () => useSecurityStore.getState().setInboxOpen(true)),
    ];
    return () => subs.forEach((s) => void s.then((off) => off()));
  }, []);

  // Voice + focus → orb; push-to-talk ← orb.
  useEffect(() => {
    const ch = openChannel();
    if (!ch) return undefined;
    let last = "";
    const publish = () => {
      const state = mainState();
      const key = JSON.stringify(state);
      if (key === last) return;
      last = key;
      safePost(ch, { type: "state", state });
    };
    const offVoice = useVoiceStore.subscribe(publish);
    const onFocus = () => publish();
    window.addEventListener("focus", onFocus);
    window.addEventListener("blur", onFocus);
    document.addEventListener("visibilitychange", onFocus);
    ch.onmessage = (e: MessageEvent<unknown>) => {
      const m = e.data as { type?: string; down?: unknown } | null;
      if (!m || isStateMessage(m)) return;
      if (m.type === "hello") {
        last = "";
        publish();
      } else if (m.type === "voice") {
        const action = (m as { action?: unknown }).action;
        if (action === "yes" || action === "no") void answerConfirmation(action === "yes");
        else if (action === "cancel") cancelVoice();
      } else if (m.type === "ptt") {
        if (m.down === true) void startListening("PushToTalk").catch((err: unknown) => console.warn("orb push-to-talk", err));
        else void stopListening().catch(() => undefined);
      }
    };
    publish();
    return () => {
      offVoice();
      window.removeEventListener("focus", onFocus);
      window.removeEventListener("blur", onFocus);
      document.removeEventListener("visibilitychange", onFocus);
      ch.onmessage = null;
      ch.close();
    };
  }, []);

  // One-time migration: robots switched off → orb off.
  useEffect(() => {
    if (!connected || !loaded) return;
    try {
      if (window.localStorage.getItem(MIGRATED_KEY)) return;
      window.localStorage.setItem(MIGRATED_KEY, "1");
    } catch {
      return;
    }
    const c = maybeClient();
    c?.call("robots.config", {}).then(
      (r) => {
        if (migratedEnabled(r.config) === false) {
          void useAiUiStore.getState().applyUserPatch([{ op: "replace", path: "/prefs/orb/enabled", value: false }], t("orb:summaries.migrated"));
        }
      },
      () => undefined,
    );
  }, [connected, loaded]);

  return null;
}
