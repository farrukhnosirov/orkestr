/** Russian rendering of the settings, onboarding, notifications, resume, search and file features. */
import type { AppSettings, Event, ResumeSummary } from "@orkestr/protocol";
import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { applyLanguagePreference } from "../i18n";
import { commands } from "./commands/registry";
import { validateName } from "./files/paths";
import { notificationFromEvent } from "./notifications/fromEvent";
import { NotificationBell } from "./notifications/NotificationCenter";
import { useNotificationsStore } from "./notifications/notificationsStore";
import { ONBOARDING_STEPS, validateFolderPath, validateProfileName } from "./onboarding/steps";
import { describeCount, headline, summaryLines } from "./resume/resume";
import { projectsProvider } from "./search/providers";
import { GeneralSection } from "./settings/sections/GeneralSection";
import { useSettingsStore } from "./settings/settingsStore";

const P = "0190f0b0-0000-7000-8000-000000000001";
const nb = (s: string) => s.replace(/ /g, " ");

function ev(partial: Pick<Event, "kind" | "payload">): Event {
  return {
    seq: 1,
    id: "0190f0b0-0000-7000-8000-0000000000bb",
    ts: "2026-10-02T10:00:00Z",
    projectId: P,
    taskId: null,
    agentId: null,
    nodeId: null,
    correlationId: null,
    ...partial,
  } as Event;
}

const SETTINGS: AppSettings = {
  profileName: "Ада",
  storageMode: "LocalOnly",
  defaultApprovalMode: "Ask",
  performanceMode: "Balanced",
  closeToTray: true,
  autostart: false,
  themeId: "nebula",
  telemetryEnabled: false,
  eventRetentionDays: 180,
  language: "ru",
  doNotDisturb: false,
  onboardingCompleted: true,
};

beforeEach(async () => {
  await act(async () => {
    await applyLanguagePreference("ru");
  });
});

afterEach(async () => {
  cleanup();
  await act(async () => {
    await applyLanguagePreference("en");
  });
});

describe("features in Russian", () => {
  it("declines resume counts and headlines", () => {
    expect(describeCount({ kind: "ApprovalRequested", count: 1 })).toBe("1 запрос на подтверждение");
    expect(describeCount({ kind: "ApprovalRequested", count: 3 })).toBe("3 запроса на подтверждение");
    expect(describeCount({ kind: "TaskCreated", count: 5 })).toBe("5 новых задач");
    expect(describeCount({ kind: "TaskCreated", count: 21 })).toBe("21 новая задача");
    expect(describeCount({ kind: "TaskCreated", count: 2 })).toBe("2 новые задачи");
    const summary: ResumeSummary = {
      since: "2026-10-01T10:00:00Z",
      previousSessionStartedAt: null,
      recoveredFromCrash: false,
      counts: [
        { kind: "FileChanged", count: 11 },
        { kind: "BuildFailed", count: 1 },
      ],
      totalEvents: 12,
      highlights: [],
    };
    expect(summaryLines(summary, 1)).toEqual(["11 изменений файлов", "и ещё 1 событие"]);
    expect(headline(summary, "Зарплаты")).toBe("12 событий в проекте «Зарплаты» с прошлого сеанса");
    expect(headline({ ...summary, totalEvents: 22 }, null)).toBe("22 события с прошлого сеанса");
    expect(headline({ ...summary, counts: [], totalEvents: 0 }, null)).toBe("С прошлого сеанса ничего не произошло");
  });

  it("writes notifications with agreement and localized numbers", () => {
    const stop = (n: number) =>
      notificationFromEvent(ev({ kind: "EmergencyStop", payload: { scope: "all", agentsStopped: n, approvalsCancelled: 0 } } as never));
    expect(stop(1)?.body).toBe("Остановлен 1 агент.");
    expect(stop(3)?.body).toBe("Остановлено 3 агента.");
    expect(stop(5)?.body).toBe("Остановлено 5 агентов.");
    const budget = notificationFromEvent(
      ev({ kind: "BudgetThreshold", payload: { scope: "project", spentUsd: 9, limitUsd: 10, fraction: 0.9 } }),
    );
    expect(nb(budget?.title ?? "")).toBe("Израсходовано 90 % бюджета");
    expect(nb(budget?.body ?? "")).toBe("project: 9,00 $ из 10,00 $");
  });

  it("translates registries, steps and validation", () => {
    expect(ONBOARDING_STEPS[0]?.title).toBe("Ваш профиль");
    expect(ONBOARDING_STEPS[9]?.title).toBe("Удалённый доступ");
    expect(validateProfileName(" ")).toBe("Введите имя (оно хранится только на этом компьютере).");
    expect(validateFolderPath("relative")).toMatch(/^Укажите абсолютный путь/);
    expect(validateName("a..")).toBe("Имя не может заканчиваться точкой или пробелом.");
    expect(projectsProvider.title).toBe("Проекты");
    const quickOpen = commands.find((c) => c.id === "files.quickOpen");
    expect(quickOpen?.title).toBe("Перейти к файлу…");
    expect(quickOpen?.group).toBe("Файлы");
  });

  it("renders the General section in Russian", () => {
    const update = vi.fn(async () => true);
    useSettingsStore.setState({ settings: SETTINGS, autostartSupported: true, loading: false, error: null, update });
    render(<GeneralSection projectId={null} />);
    expect(screen.getByRole("switch", { name: "Сворачивать в трей" })).toBeTruthy();
    expect(screen.getByRole("radio", { name: "Бессрочно" })).toBeTruthy();
    expect(screen.getByRole("radio", { name: "180 дней" })).toBeTruthy();
    expect(screen.getByRole("radio", { name: "1 год" })).toBeTruthy();
    expect(screen.getByText("Мастер настройки")).toBeTruthy();
  });

  it("labels the notification bell with a Russian plural", () => {
    const n = (id: string) => ({
      id,
      ts: "2026-10-02T10:00:00Z",
      level: "Info" as const,
      title: id,
      body: "",
      projectId: null,
      source: "local",
      read: false,
    });
    useNotificationsStore.setState({ items: [n("a"), n("b"), n("c")], toasts: [], doNotDisturb: false, centerOpen: false });
    render(<NotificationBell />);
    expect(screen.getByRole("button", { name: "Уведомления (3 непрочитанных)" })).toBeTruthy();
  });
});
