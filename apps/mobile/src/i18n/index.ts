import i18n from "i18next";
import { initReactI18next } from "react-i18next";

import type { SecureKV } from "../core";
import { en } from "./en";
import { ru } from "./ru";

export const LANGUAGES = ["en", "ru"] as const;
export type Language = (typeof LANGUAGES)[number];
const KV_LANG = "settings.lang";

void i18n.use(initReactI18next).init({
  resources: { en: { translation: en }, ru: { translation: ru } },
  lng: "en",
  fallbackLng: "en",
  interpolation: { escapeValue: false },
  returnNull: false,
});

export async function loadLanguage(kv: SecureKV, deviceLocale?: string): Promise<void> {
  const saved = await kv.get(KV_LANG).catch(() => null);
  const lang = saved ?? (deviceLocale?.toLowerCase().startsWith("ru") ? "ru" : "en");
  if (lang !== i18n.language) await i18n.changeLanguage(lang);
}

export async function setLanguage(lang: Language, kv: SecureKV): Promise<void> {
  await i18n.changeLanguage(lang);
  await kv.set(KV_LANG, lang);
}

export default i18n;
