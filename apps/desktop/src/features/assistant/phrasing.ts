/**
 * Natural replies for computer commands (ADR-0035): instead of a status
 * ("Done: Open calculator") the assistant talks like a live assistant —
 * "Opening Calculator", "Volume — 30 percent", "Setting a timer for 5
 * minutes", "Couldn't open Notepad: …"; AI-planned / multi-step runs get a
 * short summary ("Done — opened Notepad and typed the text"). Instant local
 * templates (`assistant:say.*`, a few variants each); the result card shows
 * and TTS speaks the same text.
 */
import type { ComputerAction, ComputerRunReport } from "@orkestr/protocol";

import appTable from "../../../../../crates/computer/data/apps.json";
import { currentLanguage, t as typedT } from "../../i18n";

/** Dynamic keys under `assistant:say.*` (variants are arrays). */
const t = typedT as unknown as (key: string, options?: Record<string, unknown>) => unknown;
const str = (key: string, o?: Record<string, unknown>): string => String(t(key, o) ?? "");

/** Picks variant `n` of a template (arrays hold the variants). */
function pick(key: string, vars: Record<string, unknown>, rnd: () => number): string | null {
  const v = t(`assistant:say.${key}`, { ...vars, returnObjects: true, defaultValue: "" });
  const list = Array.isArray(v) ? (v as string[]) : typeof v === "string" && v ? [v] : [];
  if (list.length === 0) return null;
  return list[Math.min(list.length - 1, Math.floor(rnd() * list.length))] ?? null;
}

function upperFirst(s: string): string {
  return s ? s.charAt(0).toLocaleUpperCase() + s.slice(1) : s;
}
function lowerFirst(s: string): string {
  return s ? s.charAt(0).toLocaleLowerCase() + s.slice(1) : s;
}
const titleCase = (s: string) => s.replace(/(^|\s)(\p{L})/gu, (_m, a: string, b: string) => a + b.toLocaleUpperCase());

interface AppNames {
  key: string;
  en: string[];
  ru: string[];
}
const APPS = (appTable as { apps: AppNames[] }).apps;
const APP_INDEX = new Map<string, AppNames>();
for (const a of APPS) for (const n of [a.key, ...a.en, ...a.ru]) APP_INDEX.set(n.toLocaleLowerCase(), a);

/**
 * Human app name in the UI language: the catalogue's display name (Russian:
 * the accusative form the catalogue phrases use — «Открываю Фигму»),
 * otherwise what was said / typed, as is.
 */
export function appName(raw: string, lang = currentLanguage()): string {
  const clean = raw.trim().replace(/\.exe$/i, "");
  const a = APP_INDEX.get(clean.toLocaleLowerCase());
  if (!a) return clean;
  return lang === "ru" ? upperFirst(a.ru[0] ?? clean) : titleCase(a.en[0] ?? clean);
}

function host(url: string): string {
  const u = url.trim().replace(/^[a-z]+:\/\//i, "").replace(/^www\./i, "");
  return u.split(/[/?#]/)[0] || url;
}
const base = (p: string) => p.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || p;
const num = (v: unknown): number | null => {
  const n = typeof v === "number" ? v : Number.parseFloat(String(v ?? ""));
  return Number.isFinite(n) ? n : null;
};

/** "5 minutes" / "1 hour 30 minutes" / "45 seconds" in the UI language. */
export function duration(seconds: number): string {
  const s = Math.max(0, Math.round(seconds));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  const parts: string[] = [];
  if (h) parts.push(str("assistant:say.unit.hours", { count: h }));
  if (m) parts.push(str("assistant:say.unit.minutes", { count: m }));
  if (sec || parts.length === 0) parts.push(str("assistant:say.unit.seconds", { count: sec }));
  return parts.join(" ");
}

/** Template key + variables of one action (null: nothing natural to say). */
function describe(a: ComputerAction): { key: string; vars: Record<string, unknown> } | null {
  switch (a.type) {
    case "Launch":
      return { key: "Launch", vars: { name: appName(a.target) } };
    case "CloseApp":
      return { key: a.discard ? "CloseAppDiscard" : "CloseApp", vars: { name: appName(a.app) } };
    case "FocusApp":
      return { key: "FocusApp", vars: { name: appName(a.app) } };
    case "KillProcess":
      return { key: "KillProcess", vars: { name: a.name } };
    case "OpenUrl":
      return { key: "OpenUrl", vars: { name: host(a.url) } };
    case "WebSearch":
      return { key: "WebSearch", vars: { query: a.query.trim() } };
    case "OpenPath":
      return { key: "OpenPath", vars: { name: base(a.path) } };
    case "Window":
      return a.app ? { key: `Window.${a.op}`, vars: { name: appName(a.app) } } : { key: `WindowSelf.${a.op}`, vars: {} };
    case "TypeText":
      return { key: "TypeText", vars: {} };
    case "Scroll":
      return { key: `Scroll.${a.direction}`, vars: {} };
    case "Volume":
    case "Brightness": {
      const v = num(a.value);
      if (a.op === "Set" && v !== null) return { key: `${a.type}.Set`, vars: { value: str("assistant:say.unit.percent", { count: v }) } };
      return { key: `${a.type}.${a.op === "Set" ? "Up" : a.op}`, vars: {} };
    }
    case "Timer": {
      const s = num(a.seconds);
      return s === null ? null : { key: "Timer", vars: { time: duration(s) } };
    }
    case "Screenshot":
      return { key: "Screenshot", vars: {} };
    case "ClipboardSet":
      return { key: "ClipboardSet", vars: {} };
    case "ClipboardGet":
      return { key: "ClipboardGet", vars: {} };
    case "FileCreate":
      return { key: "FileCreate", vars: { name: base(a.path) } };
    case "FolderCreate":
      return { key: "FolderCreate", vars: { name: base(a.path) } };
    case "FileMove":
      return { key: "FileMove", vars: { name: base(a.from) } };
    case "FileDelete":
      return { key: "FileDelete", vars: { name: base(a.path) } };
    case "Notify":
      return { key: "Notify", vars: {} };
    case "ListProcesses":
    case "ListWindows":
      return { key: a.type, vars: {} };
    default:
      return null; // Keys, Media, System, Wait, Speak, SetMode, If…: the command title says it better
  }
}

/** Steps that are worth mentioning (no waits, speech, mode switches, conditions). */
function meaningful(run: ComputerRunReport): ComputerAction[] {
  return run.steps
    .map((s) => s.step)
    .filter((s): s is ComputerAction => !!s && !["Wait", "Speak", "SetMode", "If"].includes(s.type));
}

/** Imperative command title → progressive ("Открыть блокнот" → "Открываю блокнот"). */
export function progressiveTitle(title: string): string | null {
  const m = /^(\S+)(.*)$/su.exec(title.replace(/\s*\(.*\)\s*$/, "").trim());
  if (!m) return null;
  const verbs = t("assistant:say.verbs", { returnObjects: true, defaultValue: {} }) as Record<string, string>;
  const v = verbs[m[1]!.toLocaleLowerCase()];
  return v ? upperFirst(v) + (m[2] ?? "") : null;
}

export interface PhraseOptions {
  /** Variant picker (random by default; fixed in tests). */
  rnd?: () => number;
  /** The run came from an AI plan (summary in the past tense). */
  planned?: boolean;
}

/** Natural reply for a finished run (Succeeded / Failed). */
export function naturalRunReply(run: ComputerRunReport, o: PhraseOptions = {}): string {
  const rnd = o.rnd ?? Math.random;
  const steps = meaningful(run);
  if (run.outcome === "Failed") {
    const failed = run.steps.find((s) => !s.ok);
    const error = failed?.detail ?? "";
    const d = failed?.step ? describe(failed.step) : null;
    const said = d && pick(`fail.${d.key}`, { ...d.vars, error }, rnd);
    return said || str("assistant:reply.failed", { title: run.title, error });
  }
  // several steps of an AI plan (or a user command): what was done, in short
  if ((o.planned || !run.commandId) && steps.length > 1) {
    const done: string[] = [];
    for (const s of steps) {
      const d = describe(s);
      const p = d && pick(`done.${d.key}`, d.vars, () => 0);
      if (p && !done.includes(p)) done.push(p);
    }
    if (done.length > 0) {
      const shown = done.slice(0, 3);
      const list = shown.length === 1 ? shown[0]! : `${shown.slice(0, -1).join(", ")}${str("assistant:say.and")}${shown[shown.length - 1]}`;
      const more = done.length > 3 ? str("assistant:say.andMore") : "";
      return pick("summary", { list: list + more }, rnd) ?? list;
    }
  }
  if (steps.length === 1 && steps[0]!.type === "OpenSettings") {
    // ru titles: «Настройки: экрана»; en: "Display settings"
    const colon = run.title.indexOf(":");
    const said = colon >= 0 ? pick("now.OpenSettings", { what: run.title.slice(colon + 1).trim() }, rnd) : pick("now.OpenSettingsTitled", { what: lowerFirst(run.title) }, rnd);
    if (said) return said;
  }
  if (steps.length === 1) {
    const d = describe(steps[0]!);
    const said = d && pick(`now.${d.key}`, d.vars, rnd);
    if (said) return said;
  }
  const byTitle = progressiveTitle(run.title);
  if (byTitle) return byTitle;
  return pick("generic", { what: lowerFirst(run.title) }, rnd) ?? run.title;
}
