/**
 * Locale-aware formatting on top of `Intl`. Every helper takes an optional
 * language (defaults to the current UI language) so it can be unit-tested and
 * used outside React. Inside components prefer the `useFormat()` hook, which
 * re-renders on language change.
 *
 * Russian conventions handled here: decimal comma, non-breaking space as the
 * thousands separator and before units («1,5 МБ», «5 мин»), currency after
 * the amount («1,23 $»).
 */
import i18n from "i18next";

const LOCALES: Record<string, string> = { en: "en-US", ru: "ru-RU" };

/** BCP-47 locale used by `Intl` for a UI language (`ru` → `ru-RU`). */
export function intlLocale(lng: string = i18n.language || "en"): string {
  const primary = lng.toLowerCase().split("-")[0] ?? "en";
  return LOCALES[primary] ?? lng;
}

/** Plain number, e.g. `12 345,6` (ru) / `12,345.6` (en). */
export function formatNumber(value: number, options: Intl.NumberFormatOptions = {}, lng?: string): string {
  return new Intl.NumberFormat(intlLocale(lng), options).format(value);
}

/** Integer with grouping (`1 234` / `1,234`). */
export function formatInteger(value: number, lng?: string): string {
  return formatNumber(Math.round(value), { maximumFractionDigits: 0 }, lng);
}

/** Percentage from a ratio: `0.25` → `25 %` (ru) / `25%` (en). */
export function formatPercent(ratio: number, digits = 0, lng?: string): string {
  return formatNumber(ratio, { style: "percent", maximumFractionDigits: digits }, lng);
}

/**
 * Money: `$1.23` (en) / `1,23 $` (ru). Amounts below one cent keep up to four
 * decimals so token costs never show as `$0.00`.
 */
export function formatCurrency(amount: number, currency = "USD", lng?: string): string {
  const tiny = amount !== 0 && Math.abs(amount) < 0.01;
  return new Intl.NumberFormat(intlLocale(lng), {
    style: "currency",
    currency,
    currencyDisplay: "narrowSymbol",
    minimumFractionDigits: 2,
    maximumFractionDigits: tiny ? 4 : 2,
  }).format(amount);
}

export type DateStyle = "date" | "dateLong" | "time" | "timeSeconds" | "dateTime";

const DATE_OPTIONS: Record<DateStyle, Intl.DateTimeFormatOptions> = {
  date: { dateStyle: "medium" },
  dateLong: { dateStyle: "long" },
  time: { timeStyle: "short" },
  timeSeconds: { timeStyle: "medium" },
  dateTime: { dateStyle: "medium", timeStyle: "short" },
};

function toDate(value: Date | string | number): Date {
  return value instanceof Date ? value : new Date(value);
}

/** Date and/or time in the UI locale (`2 окт. 2026 г., 14:05` / `Oct 2, 2026, 2:05 PM`). */
export function formatDate(value: Date | string | number, style: DateStyle = "dateTime", lng?: string): string {
  const d = toDate(value);
  if (Number.isNaN(d.getTime())) return String(value);
  return new Intl.DateTimeFormat(intlLocale(lng), DATE_OPTIONS[style]).format(d);
}

const RELATIVE_STEPS: ReadonlyArray<[Intl.RelativeTimeFormatUnit, number]> = [
  ["second", 60],
  ["minute", 60],
  ["hour", 24],
  ["day", 7],
  ["week", 4.345],
  ["month", 12],
  ["year", Number.POSITIVE_INFINITY],
];

/** Relative time: `5 минут назад`, `через 2 часа`, `just now` (under 45 s). */
export function formatRelativeTime(value: Date | string | number, now: number = Date.now(), lng?: string): string {
  const d = toDate(value);
  if (Number.isNaN(d.getTime())) return String(value);
  let delta = (d.getTime() - now) / 1000;
  if (Math.abs(delta) < 45) return i18n.t("common:time.justNow", { lng: lng ?? i18n.language });
  const rtf = new Intl.RelativeTimeFormat(intlLocale(lng), { numeric: "auto" });
  for (const [unit, size] of RELATIVE_STEPS) {
    if (Math.abs(delta) < size) return rtf.format(Math.round(delta), unit);
    delta /= size;
  }
  return rtf.format(Math.round(delta), "year");
}

const BYTE_UNITS = ["b", "kb", "mb", "gb", "tb"] as const;

/** Sizes in binary steps: `1,5 МБ` / `1.5 MB` (unit names from `common:units.bytes`). */
export function formatBytes(bytes: number, lng?: string): string {
  let value = Math.max(0, bytes);
  let step = 0;
  while (value >= 1024 && step < BYTE_UNITS.length - 1) {
    value /= 1024;
    step += 1;
  }
  const digits = step === 0 || value >= 100 ? 0 : 1;
  const unitName = i18n.t(`common:units.bytes.${BYTE_UNITS[step] ?? "b"}`, { lng: lng ?? i18n.language });
  return `${formatNumber(value, { maximumFractionDigits: digits }, lng)}\u00a0${unitName}`;
}

function unit(value: number, name: "hour" | "minute" | "second" | "millisecond", lng?: string): string {
  return formatNumber(value, { style: "unit", unit: name, unitDisplay: "short", maximumFractionDigits: 0 }, lng);
}

/**
 * Durations: `450 мс`, `12 с`, `3 мин 5 с`, `2 ч 15 мин` (en: `450 ms`,
 * `12 sec`, `3 min 5 sec`, `2 hr 15 min`). Shows the two largest units.
 */
export function formatDuration(ms: number, lng?: string): string {
  const total = Math.max(0, ms);
  if (total < 1000) return unit(Math.round(total), "millisecond", lng);
  if (total < 10_000) return formatNumber(total / 1000, { style: "unit", unit: "second", unitDisplay: "short", maximumFractionDigits: 1 }, lng);
  const secs = Math.round(total / 1000);
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = secs % 60;
  if (h > 0) return m > 0 ? `${unit(h, "hour", lng)} ${unit(m, "minute", lng)}` : unit(h, "hour", lng);
  if (m > 0) return s > 0 ? `${unit(m, "minute", lng)} ${unit(s, "second", lng)}` : unit(m, "minute", lng);
  return unit(s, "second", lng);
}

/** `a, b и c` / `a, b, and c`. */
export function formatList(items: readonly string[], type: "conjunction" | "disjunction" = "conjunction", lng?: string): string {
  return new Intl.ListFormat(intlLocale(lng), { style: "long", type }).format(items);
}

/** Bound helpers for the current language (see `useFormat`). */
export interface Formatters {
  number: (value: number, options?: Intl.NumberFormatOptions) => string;
  integer: (value: number) => string;
  percent: (ratio: number, digits?: number) => string;
  currency: (amount: number, currency?: string) => string;
  date: (value: Date | string | number, style?: DateStyle) => string;
  relative: (value: Date | string | number, now?: number) => string;
  bytes: (bytes: number) => string;
  duration: (ms: number) => string;
  list: (items: readonly string[], type?: "conjunction" | "disjunction") => string;
}

/** Formatters bound to `lng`. */
export function formattersFor(lng: string): Formatters {
  return {
    number: (v, o) => formatNumber(v, o, lng),
    integer: (v) => formatInteger(v, lng),
    percent: (r, d) => formatPercent(r, d, lng),
    currency: (a, c) => formatCurrency(a, c, lng),
    date: (v, s) => formatDate(v, s, lng),
    relative: (v, n) => formatRelativeTime(v, n, lng),
    bytes: (b) => formatBytes(b, lng),
    duration: (ms) => formatDuration(ms, lng),
    list: (items, type) => formatList(items, type, lng),
  };
}
