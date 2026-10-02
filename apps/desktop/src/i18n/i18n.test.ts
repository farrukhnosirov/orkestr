import { OrkestrError } from "@orkestr/client";
import { afterEach, describe, expect, it } from "vitest";

import { describeError } from "./errors";
import {
  applyLanguagePreference,
  formatBytes,
  formatCurrency,
  formatDate,
  formatDuration,
  formatNumber,
  formatRelativeTime,
  i18n,
  resolveLanguage,
  systemLanguage,
  t,
  toPreference,
} from "./index";

const NBSP = /[  ]/g;
const plain = (s: string) => s.replace(NBSP, " ");

afterEach(async () => {
  await applyLanguagePreference("en");
});

describe("language resolution", () => {
  it("maps the system language: Russian → ru, anything else → en", () => {
    expect(systemLanguage(["ru-RU", "en-US"])).toBe("ru");
    expect(systemLanguage(["en-GB"])).toBe("en");
    expect(systemLanguage(["de-DE"])).toBe("en");
    expect(systemLanguage(["de-DE", "ru"])).toBe("ru");
    expect(systemLanguage([])).toBe("en");
  });

  it("resolves stored preferences", () => {
    expect(resolveLanguage("ru", ["en-US"])).toBe("ru");
    expect(resolveLanguage("en", ["ru-RU"])).toBe("en");
    expect(resolveLanguage("system", ["ru-RU"])).toBe("ru");
    expect(resolveLanguage("de", ["ru-RU"])).toBe("ru");
    expect(resolveLanguage(null, ["fr"])).toBe("en");
    expect(toPreference("ru")).toBe("ru");
    expect(toPreference("de")).toBe("system");
  });

  it("switches live and sets the language of global t()", async () => {
    expect(t("shell:commands.projectCreate")).toBe("Create Project");
    await applyLanguagePreference("ru");
    expect(i18n.language).toBe("ru");
    expect(t("shell:commands.projectCreate")).toBe("Создать проект");
    expect(document.documentElement.lang === "ru" || document.documentElement.lang === "").toBe(true);
  });
});

describe("Russian plurals", () => {
  it("uses one / few / many", async () => {
    await applyLanguagePreference("ru");
    expect(t("common:count.agents", { count: 1 })).toBe("1 агент");
    expect(t("common:count.agents", { count: 2 })).toBe("2 агента");
    expect(t("common:count.agents", { count: 5 })).toBe("5 агентов");
    expect(t("common:count.agents", { count: 11 })).toBe("11 агентов");
    expect(t("common:count.agents", { count: 21 })).toBe("21 агент");
    expect(t("common:count.agents", { count: 1.5 })).toBe("1.5 агента");
    expect(t("shell:stopAll.agents", { count: 3 })).toBe("остановлено 3 агента");
  });

  it("uses one / other in English", () => {
    expect(t("common:count.agents", { count: 1 })).toBe("1 agent");
    expect(t("common:count.agents", { count: 2 })).toBe("2 agents");
  });
});

describe("formatters", () => {
  it("formats money per locale", () => {
    expect(formatCurrency(1.23, "USD", "en")).toBe("$1.23");
    expect(plain(formatCurrency(1.23, "USD", "ru"))).toBe("1,23 $");
    expect(formatCurrency(0.0042, "USD", "en")).toBe("$0.0042");
  });

  it("formats numbers, bytes and durations", () => {
    expect(plain(formatNumber(12345.6, {}, "ru"))).toBe("12 345,6");
    expect(formatNumber(12345.6, {}, "en")).toBe("12,345.6");
    expect(plain(formatBytes(1536 * 1024, "ru"))).toBe("1,5 МБ");
    expect(plain(formatBytes(1536 * 1024, "en"))).toBe("1.5 MB");
    expect(plain(formatBytes(512, "en"))).toBe("512 B");
    expect(plain(formatBytes(3 * 1024 ** 3, "ru"))).toBe("3 ГБ");
    expect(plain(formatDuration(450, "ru"))).toBe("450 мс");
    expect(plain(formatDuration(185_000, "ru"))).toBe("3 мин 5 с");
    expect(plain(formatDuration(8_100_000, "en"))).toBe("2 hr 15 min");
  });

  it("formats relative time", () => {
    const now = Date.parse("2026-10-02T12:00:00Z");
    expect(formatRelativeTime(now - 5 * 60_000, now, "ru")).toBe("5 минут назад");
    expect(formatRelativeTime(now - 5 * 60_000, now, "en")).toBe("5 minutes ago");
    expect(formatRelativeTime(now + 2 * 3_600_000, now, "ru")).toBe("через 2 часа");
    expect(formatRelativeTime(now - 10_000, now, "ru")).toBe("только что");
    expect(formatRelativeTime(now - 26 * 3_600_000, now, "ru")).toBe("вчера");
  });

  it("formats dates", () => {
    const ts = "2026-10-02T09:05:00Z";
    expect(formatDate(ts, "date", "ru")).toMatch(/2 окт\. 2026/);
    expect(formatDate(ts, "date", "en")).toMatch(/Oct 2, 2026/);
  });

  it("supports formatters inside strings", async () => {
    await applyLanguagePreference("ru");
    i18n.addResource("ru", "common", "__probe", "{{size, bytes}} за {{ms, duration}}, {{cost, usd}}");
    expect(plain(i18n.t("common:__probe" as "common:brand", { size: 2048, ms: 1500, cost: 2.5 }))).toBe("2 КБ за 1,5 с, 2,50 $");
  });
});

describe("errors", () => {
  it("localizes RPC error codes and keeps the detail", () => {
    const e = new OrkestrError("not_found", "project 42 not found");
    expect(describeError(e, "en")).toBe("project 42 not found");
    expect(describeError(e, "ru")).toBe("Не найдено: project 42 not found");
    expect(describeError(new OrkestrError("budget_exceeded", ""), "ru")).toBe("Бюджет исчерпан");
    expect(describeError(new Error("boom"), "ru")).toBe("boom");
  });
});
