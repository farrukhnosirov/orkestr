/**
 * Voice Command Center + Sound Engine (§33, §63 G, ADR-0017) — public API.
 *
 * - `useVoiceBootstrap(ready)`: mount once in the app shell.
 * - `VoiceHud`, `MicIndicator`: overlay + title-bar indicator.
 * - `voiceSettingsSections`, `voiceCommands`: registry contributions.
 * - `speakAsAgent(agentId, text)`: voice per agent for robots / bubbles.
 * - `playSound(cue)`: procedural UI sounds (re-exported from features/sound).
 */
import { AudioLines, Mic } from "lucide-react";

import { t } from "../../i18n";
import type { AppCommand } from "../commands/registry";
import type { SettingsSection } from "../settings/sections";
import { useSettingsStore } from "../settings/settingsStore";
import { SoundSection } from "./settings/SoundSection";
import { VoiceSection } from "./settings/VoiceSection";
import { useVoiceStore } from "./store";

export { speakAsAgent, voiceProfileForAgent } from "./agentVoices";
export { MicIndicator } from "./ui/MicIndicator";
export { VoiceHud } from "./ui/VoiceHud";
export { useVoiceBootstrap } from "./useVoiceBootstrap";
export { playSound } from "../sound/engine";
export type { SoundCue } from "../sound/recipes";

export const voiceSettingsSection: SettingsSection = {
  id: "voice",
  get title() {
    return t("voice:settings.title");
  },
  icon: Mic,
  order: 300,
  component: VoiceSection,
};

export const soundSettingsSection: SettingsSection = {
  id: "sound",
  get title() {
    return t("voice:sound.title");
  },
  icon: AudioLines,
  order: 310,
  component: SoundSection,
};

export const voiceCommands: AppCommand[] = [
  {
    id: "voice.open",
    get title() {
      return t("voice:commands.open");
    },
    get group() {
      return t("voice:commands.group");
    },
    icon: Mic,
    keywords: ["voice", "speak", "talk", "jarvis", "microphone", "голос", "голосовые команды", "микрофон", "сказать"],
    enabled: () => useVoiceStore.getState().config?.enabled ?? false,
    run: () => useVoiceStore.getState().setHudOpen(true),
  },
  {
    id: "voice.settings",
    get title() {
      return t("voice:commands.settings");
    },
    get group() {
      return t("voice:commands.group");
    },
    icon: Mic,
    keywords: ["voice", "wake word", "tts", "stt", "голос", "фраза активации", "озвучка", "распознавание речи"],
    run: () => useSettingsStore.getState().openSettings("voice"),
  },
  {
    id: "voice.toggleSounds",
    get title() {
      return useVoiceStore.getState().config?.sound.enabled ? t("voice:commands.soundsOff") : t("voice:commands.soundsOn");
    },
    get group() {
      return t("voice:commands.group");
    },
    icon: AudioLines,
    keywords: ["sound", "mute", "audio", "звук", "звуки", "выключить звук"],
    enabled: () => useVoiceStore.getState().config !== null,
    run: () => void useVoiceStore.getState().patch((c) => ({ ...c, sound: { ...c.sound, enabled: !c.sound.enabled } })),
  },
];
