/**
 * Jarvis persona (ADR-0049): the form of address and the persona phrases
 * every speaking part of the app uses.
 *
 * - `{{addr}}` in any translation is the address in suffix form
 *   («Открываю YouTube{{addr}}.» → «Открываю YouTube, сэр.»). It is an
 *   i18next default variable kept in sync with the Jarvis settings and the
 *   UI language by `applyPersona()`; persona off / no address → "".
 * - `jarvisSay(kind, vars)`: a persona phrase (`jarvis:say.<kind>`, several
 *   variants each) that never repeats the previous variant of that kind.
 * - `timeOfDay()` / `greeting()`: «Доброе утро, сэр» by the local clock.
 */
import type { JarvisAddress, JarvisConfig } from "@orkestr/protocol";

import { currentLanguage, i18n, t as typedT } from "../../i18n";

/** Dynamic keys under `jarvis:say.*` (arrays of variants). */
const t = typedT as unknown as (key: string, options?: Record<string, unknown>) => unknown;

/** Persona phrase kinds (`jarvis:say.<kind>`). */
export type JarvisSayKind =
  | "greet.morning"
  | "greet.day"
  | "greet.evening"
  | "greet.night"
  | "welcome"
  | "listening"
  | "modeChat"
  | "modeCommands"
  | "forgot"
  | "addressSet"
  | "noModel"
  | "aiFailed"
  | "notUnderstood"
  | "nothingToRepeat"
  | "done"
  | "cancelled"
  | "handoff";

export type TimeOfDay = "morning" | "day" | "evening" | "night";

/** What the persona needs from the settings (defaults: persona on, «сэр»). */
export type PersonaSettings = Pick<JarvisConfig, "persona" | "address" | "userName">;

export const DEFAULT_PERSONA: PersonaSettings = { persona: true, address: "Sir", userName: "" };

let current: PersonaSettings = DEFAULT_PERSONA;
const lastVariant = new Map<string, number>();

/** The address word in the UI language («сэр», "boss", the name) or "" for none. */
export function addressWord(p: PersonaSettings = current, lang: string = currentLanguage()): string {
  if (!p.persona) return "";
  const a: JarvisAddress = p.address;
  if (a === "Name") return p.userName.trim();
  if (a === "None") return "";
  return String(t(`jarvis:addressWord.${a}`, { lng: lang, defaultValue: a === "Sir" ? "sir" : "boss" }));
}

/** Suffix form: ", сэр" / "". */
export function addressSuffix(p: PersonaSettings = current, lang: string = currentLanguage()): string {
  const w = addressWord(p, lang);
  return w ? `, ${w}` : "";
}

/** The persona settings in effect. */
export function personaSettings(): PersonaSettings {
  return current;
}

/**
 * Applies the persona: stores the settings and refreshes the `{{addr}}`
 * default variable for the current language. Call on settings load / change
 * and on language change.
 */
export function applyPersona(p: PersonaSettings = current): void {
  current = { persona: p.persona, address: p.address, userName: p.userName };
  const interpolation = (i18n.options.interpolation ??= {});
  interpolation.defaultVariables = { ...(interpolation.defaultVariables ?? {}), addr: addressSuffix(current) };
}

let languageHooked = false;
/** Keeps `{{addr}}` in the UI language («сэр» / "sir"). */
export function hookPersonaLanguage(): void {
  if (languageHooked) return;
  languageHooked = true;
  i18n.on("languageChanged", () => applyPersona());
}

/** Picks a variant other than the previous one of `key`. */
export function pickVariant<T>(key: string, list: readonly T[], rnd: () => number = Math.random): T | undefined {
  if (list.length === 0) return undefined;
  const prev = lastVariant.get(key);
  let i = Math.min(list.length - 1, Math.floor(rnd() * list.length));
  if (list.length > 1 && i === prev) i = (i + 1) % list.length;
  lastVariant.set(key, i);
  return list[i];
}

/** Test seam: forgets the previous variants. */
export function resetVariants(): void {
  lastVariant.clear();
}

/**
 * A persona phrase: `jarvisSay("modeChat")` → «Режим разговора, сэр. Я весь
 * внимание.» Variables are interpolated (`{{error}}`, …); `{{addr}}` comes
 * from the settings.
 */
export function jarvisSay(kind: JarvisSayKind, vars: Record<string, unknown> = {}, rnd: () => number = Math.random): string {
  const v = t(`jarvis:say.${kind}`, { ...vars, returnObjects: true, defaultValue: "" });
  const list = Array.isArray(v) ? (v as string[]) : typeof v === "string" && v ? [v] : [];
  return pickVariant(`say:${kind}`, list, rnd) ?? "";
}

/** Part of the day by the local clock. */
export function timeOfDay(now: Date = new Date()): TimeOfDay {
  const h = now.getHours();
  if (h >= 5 && h < 12) return "morning";
  if (h >= 12 && h < 18) return "day";
  if (h >= 18 && h < 23) return "evening";
  return "night";
}

/** «Доброе утро, сэр.» / "Good evening, sir." */
export function greeting(now: Date = new Date(), rnd: () => number = Math.random): string {
  return jarvisSay(`greet.${timeOfDay(now)}`, {}, rnd);
}

const GREETED_KEY = "orkestr.jarvis.greetedOn";

/** Local calendar day `YYYY-MM-DD`. */
function dayKey(now: Date): string {
  const p = (n: number) => String(n).padStart(2, "0");
  return `${now.getFullYear()}-${p(now.getMonth() + 1)}-${p(now.getDate())}`;
}

/**
 * The greeting for the first start / first wake of the day, or null when the
 * user was already greeted today (remembered on this device).
 */
export function firstGreetingOfDay(now: Date = new Date(), rnd: () => number = Math.random): string | null {
  const today = dayKey(now);
  try {
    if (window.localStorage.getItem(GREETED_KEY) === today) return null;
    window.localStorage.setItem(GREETED_KEY, today);
  } catch {
    /* storage unavailable: greet anyway */
  }
  return greeting(now, rnd);
}
