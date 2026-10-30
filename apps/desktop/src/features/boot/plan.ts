/**
 * What plays on this start (decided synchronously before the first paint),
 * the texts of the sequence, and the mirror that keeps the boot environment
 * (sound, quiet hours, performance mode, form of address) for the next start.
 */
import { useEffect } from "react";

import pkg from "../../../package.json";
import { currentLanguage, t } from "../../i18n";
import { addressSuffix, timeOfDay } from "../jarvis/persona";
import { useJarvisStore } from "../jarvis/store";
import { usePerfStore } from "../performance/perfStore";
import { useSettingsStore } from "../settings/settingsStore";
import { useVoiceStore } from "../voice/store";
import { isFirstLaunch, readBootEnv, readBootPrefs, readLastVersion, shortVersion, writeBootEnv, type BootEnv } from "./prefs";
import { chooseVariant, type BootVariant } from "./sequence";

/** This build's version (`apps/desktop/package.json`, bumped with every release). */
export const APP_VERSION: string = pkg.version;

export interface BootPlan {
  variant: BootVariant;
  firstLaunch: boolean;
  env: BootEnv;
}

function reducedMotion(): boolean {
  try {
    return typeof window.matchMedia === "function" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  } catch {
    return false;
  }
}

function hiddenAtStart(): boolean {
  return typeof document !== "undefined" && document.visibilityState === "hidden";
}

/** Decides the version for this start from the device's stored state. */
export function planBoot(): BootPlan {
  const env = readBootEnv();
  const prefs = readBootPrefs();
  const firstLaunch = isFirstLaunch(APP_VERSION, readLastVersion());
  const variant = chooseVariant({
    pref: prefs.animation,
    firstLaunch,
    reducedMotion: reducedMotion(),
    performanceMode: env.performanceMode,
    hidden: hiddenAtStart(),
  });
  return { variant, firstLaunch, env };
}

/** «, сэр» from the mirrored Jarvis settings, in the UI language. */
export function bootAddress(env: BootEnv): string {
  return addressSuffix({ persona: env.persona, address: env.address, userName: env.userName }, currentLanguage());
}

/** Texts of the sequence in the UI language. */
export function bootTexts(env: BootEnv, now: Date = new Date()) {
  const addr = bootAddress(env);
  const tod = timeOfDay(now);
  const clock = now.toLocaleTimeString(currentLanguage(), { hour: "2-digit", minute: "2-digit" });
  const date = now.toLocaleDateString(currentLanguage(), { weekday: "short", day: "numeric", month: "short" });
  return {
    title: t("boot:title", { version: shortVersion(APP_VERSION) }),
    steps: {
      core: t("boot:steps.core"),
      providers: t("boot:steps.providers"),
      voice: t("boot:steps.voice"),
      nodes: t("boot:steps.nodes"),
      plugins: t("boot:steps.plugins"),
    },
    allOnline: t("boot:allOnline"),
    waiting: t("boot:waiting"),
    welcome: t("boot:welcome", { addr }),
    subtitle: t(`boot:subtitle.${tod}`, { time: clock, date }),
    spoken: t(`boot:spoken.${tod}`, { addr }),
    skip: t("boot:skip"),
    status: { ok: t("boot:status.ok"), pending: t("boot:status.pending"), skipped: t("boot:status.skipped"), error: t("boot:status.error") },
  };
}

/** Keeps the boot environment mirror up to date while the app runs. */
export function useBootMirror(ready: boolean): void {
  useEffect(() => {
    if (!ready) return undefined;
    const sync = () => {
      const voice = useVoiceStore.getState().config;
      const settings = useSettingsStore.getState().settings;
      const jarvis = useJarvisStore.getState().config;
      const patch: Partial<BootEnv> = { performanceMode: usePerfStore.getState().chosen };
      if (settings) {
        patch.doNotDisturb = settings.doNotDisturb;
        patch.performanceMode = settings.performanceMode;
      }
      if (voice) {
        patch.soundEnabled = voice.sound.enabled;
        patch.volumePct = voice.sound.volumePct;
        patch.muteInDnd = voice.sound.muteInDnd;
        patch.quietHours = voice.notifications.quietHours;
      }
      if (jarvis) {
        patch.persona = jarvis.persona;
        patch.address = jarvis.address;
        patch.userName = jarvis.userName;
      }
      writeBootEnv(patch);
    };
    sync();
    const offs = [
      useVoiceStore.subscribe((s, p) => s.config !== p.config && sync()),
      useSettingsStore.subscribe((s, p) => s.settings !== p.settings && sync()),
      useJarvisStore.subscribe((s, p) => s.config !== p.config && sync()),
    ];
    return () => offs.forEach((off) => off());
  }, [ready]);
}
