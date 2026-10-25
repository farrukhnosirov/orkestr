/**
 * Registers Jarvis (ADR-0049): Settings → Jarvis, palette commands
 * (conversation / command mode, forget the conversation) and the bootstrap
 * (settings + persona, language hook, greeting on start).
 */
import { Eraser, MessageCircle, UserRound } from "lucide-react";
import { useEffect } from "react";

import { t } from "../../i18n";
import { useAssistantStore } from "../assistant/store";
import { commands } from "../commands/registry";
import { settingsSections } from "../settings/sections";
import { useSettingsStore } from "../settings/settingsStore";
import { say } from "../voice/controller";
import { useVoiceStore } from "../voice/store";
import { forgetConversation, loadJarvisConfig } from "./api";
import { dailyGreeting } from "./greeting";
import { JarvisSection } from "./JarvisSection";
import { startJarvisModeSync } from "./modeSync";
import { hookPersonaLanguage, jarvisSay } from "./persona";
import { useJarvisStore } from "./store";

let registered = false;

export function registerJarvisFeature(): void {
  if (registered) return;
  registered = true;
  // the Jarvis circle shows chat / command / cursor (ADR-0052)
  startJarvisModeSync();
  settingsSections.push({
    id: "jarvis",
    get title() {
      return t("jarvis:settings.title");
    },
    icon: UserRound,
    order: 306,
    component: JarvisSection,
  });
  commands.push(
    {
      id: "jarvis.toggleChat",
      get title() {
        return t("jarvis:commands.toggleChat");
      },
      get group() {
        return t("jarvis:commands.group");
      },
      icon: MessageCircle,
      keywords: ["jarvis", "chat", "conversation", "talk", "джарвис", "разговор", "беседа", "режим"],
      run: () => {
        const s = useJarvisStore.getState();
        const next = s.mode === "chat" ? "commands" : "chat";
        s.setMode(next);
        const text = jarvisSay(next === "chat" ? "modeChat" : "modeCommands");
        useAssistantStore.getState().patch({ reply: { text, tone: "info" } });
      },
    },
    {
      id: "jarvis.forget",
      get title() {
        return t("jarvis:commands.forget");
      },
      get group() {
        return t("jarvis:commands.group");
      },
      icon: Eraser,
      keywords: ["jarvis", "forget", "history", "clear", "джарвис", "забудь", "история", "очистить"],
      run: () => forgetConversation(),
    },
    {
      id: "jarvis.settings",
      get title() {
        return t("jarvis:commands.settings");
      },
      get group() {
        return t("jarvis:commands.group");
      },
      icon: UserRound,
      keywords: ["jarvis", "persona", "sir", "boss", "джарвис", "сэр", "босс", "обращение"],
      run: () => useSettingsStore.getState().openSettings("jarvis"),
    },
  );
}

/** Greets once the voice settings are known (spoken replies on / off). */
function greetOnStart(): () => void {
  let done = false;
  const tryGreet = () => {
    if (done || !useVoiceStore.getState().config) return;
    done = true;
    const text = dailyGreeting();
    if (!text) return;
    useAssistantStore.getState().patch({ reply: { text, tone: "info" } });
    useJarvisStore.getState().patch({ lastReply: text });
    void say(text);
  };
  const off = useVoiceStore.subscribe(tryGreet);
  tryGreet();
  return off;
}

/** Mount once in the app shell (after the core connection is ready). */
export function useJarvisBootstrap(ready: boolean): void {
  useEffect(() => {
    if (!ready) return;
    hookPersonaLanguage();
    let off: (() => void) | null = null;
    let alive = true;
    void loadJarvisConfig().then(() => {
      if (alive) off = greetOnStart();
    });
    return () => {
      alive = false;
      off?.();
    };
  }, [ready]);
}
