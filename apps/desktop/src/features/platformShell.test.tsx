import type { AppSettings, Event, ResumeSummary } from "@orkestr/protocol";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { groupItems, type PaletteItem, rankItems } from "../components/CommandPalette";
import { commands } from "./commands/registry";
import { notificationFromEvent, shouldToast, type AppNotification } from "./notifications/fromEvent";
import { relativeTime } from "./notifications/NotificationCenter";
import { MAX_HISTORY, unreadCount, useNotificationsStore } from "./notifications/notificationsStore";
import {
  canFinishAt,
  LAST_ACTIVE_STEP,
  ONBOARDING_STEPS,
  stepState,
  validateFolderPath,
  validateProfileName,
} from "./onboarding/steps";
import { describeCount, headline, humanizeKind, summaryLines } from "./resume/resume";
import { fuzzyMatch, fuzzyScore, highlightSegments } from "./search/fuzzy";
import { pushRecent } from "./search/paletteStore";
import { formatBytes, formatDuration, Segmented, Switch } from "./settings/controls";
import { orderedSections, settingsSections, type SettingsSection } from "./settings/sections";
import { GeneralSection } from "./settings/sections/GeneralSection";
import { useSettingsStore } from "./settings/settingsStore";
import { matchesShortcut } from "./commands/registry";
import { notificationTopics } from "./settings/useShellBootstrap";

const P = "0190f0b0-0000-7000-8000-000000000001";

function ev(partial: Partial<Event> & Pick<Event, "kind" | "payload">): Event {
  return {
    seq: 7,
    id: "0190f0b0-0000-7000-8000-0000000000aa",
    ts: "2026-10-02T10:00:00Z",
    projectId: P,
    taskId: null,
    agentId: null,
    nodeId: null,
    correlationId: null,
    ...partial,
  } as Event;
}

const DEFAULT_SETTINGS: AppSettings = {
  profileName: "",
  storageMode: "LocalOnly",
  defaultApprovalMode: "Ask",
  performanceMode: "Balanced",
  closeToTray: true,
  autostart: false,
  themeId: "nebula",
  telemetryEnabled: false,
  eventRetentionDays: 180,
  language: "en",
  doNotDisturb: false,
  onboardingCompleted: false,
};

describe("fuzzy search", () => {
  it("prefers substrings and word starts and reports positions", () => {
    const m = fuzzyMatch("set", "Open Settings");
    expect(m?.indices).toEqual([5, 6, 7]);
    expect(fuzzyScore("set", "Open Settings")).toBeGreaterThan(fuzzyScore("ots", "Open Settings"));
    expect(fuzzyMatch("xyz", "Open Settings")).toBeNull();
    expect(fuzzyScore("", "anything")).toBe(0);
    const sub = fuzzyMatch("opst", "Open Settings");
    expect(sub?.indices).toEqual([0, 1, 5, 7]);
  });

  it("splits highlight segments", () => {
    expect(highlightSegments("Logs", [0, 1])).toEqual([
      { text: "Lo", match: true },
      { text: "gs", match: false },
    ]);
    expect(highlightSegments("abc", [])).toEqual([{ text: "abc", match: false }]);
  });
});

describe("command palette", () => {
  const items: PaletteItem[] = [
    { id: "a", title: "Open Settings", group: "General", indices: [], run: () => {} },
    { id: "b", title: "Open Logs", group: "Views", keywords: "events stream", indices: [], run: () => {} },
    { id: "c", title: "Stop All AI", group: "Safety", keywords: "emergency", disabled: true, indices: [], run: () => {} },
  ];

  it("ranks by title, then keywords, disabled last", () => {
    expect(rankItems(items, "logs").map((i) => i.id)).toEqual(["b"]);
    expect(rankItems(items, "events").map((i) => i.id)).toEqual(["b"]);
    expect(rankItems(items, "emergency").map((i) => i.id)).toEqual(["c"]);
    // Both start with "Open"; the shorter title ranks first.
    expect(rankItems(items, "open").map((i) => i.id)).toEqual(["b", "a"]);
    expect(rankItems(items, "").map((i) => i.id)).toEqual(["a", "b", "c"]);
    expect(rankItems(items, "settings")[0]?.indices.length).toBe(8);
  });

  it("groups in first-appearance order", () => {
    expect(groupItems(items).map((g) => g.group)).toEqual(["General", "Views", "Safety"]);
  });

  it("registers the core commands", () => {
    const ids = commands.map((c) => c.id);
    for (const id of ["project.create", "project.switch", "settings.open", "view.missionControl", "view.logs", "window.fullscreen", "ai.stopAll"]) {
      expect(ids).toContain(id);
    }
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("keeps recents unique and capped", () => {
    expect(pushRecent(["a", "b", "c"], "b")).toEqual(["b", "a", "c"]);
    expect(pushRecent(["a", "b", "c"], "d", 3)).toEqual(["d", "a", "b"]);
  });

  it("maps shell shortcuts through the command registry", () => {
    const ev = (key: string, mods: Partial<KeyboardEvent> = {}) => ({ key, code: "", ctrlKey: false, metaKey: false, shiftKey: false, altKey: false, ...mods });
    const hit = (e: ReturnType<typeof ev>) => commands.find((c) => c.shortcut && matchesShortcut(e, c.shortcut))?.id ?? null;
    expect(hit(ev(",", { ctrlKey: true }))).toBe("settings.open");
    expect(hit(ev("p", { ctrlKey: true, altKey: true }))).toBe("project.switch");
    expect(hit(ev("p", { ctrlKey: true }))).toBe("files.quickOpen");
    expect(hit(ev("F11"))).toBe("window.fullscreen");
    expect(hit(ev("Backspace", { ctrlKey: true, shiftKey: true }))).toBe("ai.stopAll");
    expect(hit(ev("k", { ctrlKey: true }))).toBeNull();
  });
});

describe("notifications", () => {
  beforeEach(() => useNotificationsStore.setState({ items: [], toasts: [], doNotDisturb: false }));

  it("maps important events", () => {
    const failed = notificationFromEvent(
      ev({ kind: "TaskStatusChanged", payload: { taskId: P, from: "Running", to: "Failed", reason: "tests failed" } }),
    );
    expect(failed).toMatchObject({ level: "Error", title: "Task failed", body: "tests failed", projectId: P });
    expect(
      notificationFromEvent(ev({ kind: "TaskStatusChanged", payload: { taskId: P, from: "Ready", to: "Running", reason: null } })),
    ).toBeNull();
    const budget = notificationFromEvent(
      ev({ kind: "BudgetThreshold", payload: { scope: "project", spentUsd: 9, limitUsd: 10, fraction: 0.9 } }),
    );
    expect(budget?.title).toBe("Budget 90% used");
    const fallback = notificationFromEvent(
      ev({
        kind: "ProviderFallback",
        payload: {
          from: { providerId: P, model: "gpt-primary" },
          to: { providerId: P, model: "mock-smart" },
          reason: "quota_exceeded",
        },
      }),
    );
    expect(fallback).toMatchObject({ level: "Warning", title: "Switched to the fallback model" });
    expect(fallback?.body).toBe("gpt-primary → mock-smart: quota exhausted. The task continues from a saved handoff.");
    expect(notificationFromEvent(ev({ kind: "Log", payload: { level: "info", target: "t", message: "m", fields: null } }))).toBeNull();
  });

  it("dedupes, honours do-not-disturb and caps history", () => {
    const n = (id: string, level: AppNotification["level"] = "Info"): AppNotification => ({
      id,
      ts: "2026-10-02T10:00:00Z",
      level,
      title: id,
      body: "",
      projectId: null,
      source: "local",
      read: false,
    });
    const s = useNotificationsStore.getState();
    s.push(n("1"));
    s.push(n("1"));
    expect(useNotificationsStore.getState().items).toHaveLength(1);
    expect(useNotificationsStore.getState().toasts).toHaveLength(1);
    s.setDoNotDisturb(true);
    expect(useNotificationsStore.getState().toasts).toHaveLength(0);
    s.push(n("2"));
    s.push(n("3", "Critical"));
    expect(useNotificationsStore.getState().toasts.map((t) => t.id)).toEqual(["3"]);
    expect(unreadCount(useNotificationsStore.getState().items)).toBe(3);
    s.markAllRead();
    expect(unreadCount(useNotificationsStore.getState().items)).toBe(0);
    for (let i = 0; i < MAX_HISTORY + 10; i++) s.push(n(`x${i}`));
    expect(useNotificationsStore.getState().items).toHaveLength(MAX_HISTORY);
    expect(shouldToast(n("y"), true)).toBe(false);
    expect(shouldToast(n("z", "Critical"), true)).toBe(true);
    // Approvals are toasted once, by the actionable security toast.
    expect(shouldToast({ ...n("a"), source: "ApprovalRequested" }, false)).toBe(false);
  });

  it("formats relative times", () => {
    const now = Date.parse("2026-10-02T10:10:00Z");
    expect(relativeTime("2026-10-02T10:09:50Z", now)).toBe("just now");
    expect(relativeTime("2026-10-02T10:00:00Z", now)).toBe("10 minutes ago");
    expect(relativeTime("2026-10-02T07:00:00Z", now)).toBe("3 hours ago");
  });

  it("subscribes to global and every project", () => {
    expect(notificationTopics([P])).toEqual(["global", `project:${P}`]);
  });
});

describe("onboarding steps", () => {
  it("has the 12 §55 steps with later phases marked", () => {
    expect(ONBOARDING_STEPS).toHaveLength(12);
    expect(ONBOARDING_STEPS.slice(8).every((s) => s.phase > 1)).toBe(true);
    expect(ONBOARDING_STEPS[LAST_ACTIVE_STEP]?.id).toBe("terminal");
    expect(canFinishAt(LAST_ACTIVE_STEP)).toBe(true);
    expect(canFinishAt(0)).toBe(false);
  });

  it("derives stepper states", () => {
    const visited = new Set([0, 1]);
    expect(stepState(0, 2, visited)).toBe("done");
    expect(stepState(2, 2, visited)).toBe("current");
    expect(stepState(3, 2, visited)).toBe("upcoming");
    expect(stepState(10, 2, visited)).toBe("later");
  });

  it("validates inputs like the core", () => {
    expect(validateProfileName("  ")).not.toBeNull();
    expect(validateProfileName("Ada")).toBeNull();
    expect(validateProfileName("x".repeat(81))).not.toBeNull();
    expect(validateFolderPath("")).toBeNull();
    expect(validateFolderPath("C:\\Code\\app")).toBeNull();
    expect(validateFolderPath("/home/me/app")).toBeNull();
    expect(validateFolderPath("relative/path")).not.toBeNull();
  });
});

describe("session resume", () => {
  const summary: ResumeSummary = {
    since: "2026-10-01T10:00:00Z",
    previousSessionStartedAt: "2026-10-01T10:00:00Z",
    recoveredFromCrash: false,
    counts: [
      { kind: "TaskStatusChanged", count: 5 },
      { kind: "ApprovalRequested", count: 1 },
      { kind: "SomethingNew", count: 2 },
    ],
    totalEvents: 8,
    highlights: [],
  };

  it("describes counts generically", () => {
    expect(describeCount({ kind: "ApprovalRequested", count: 1 })).toBe("1 approval request");
    expect(describeCount({ kind: "TaskStatusChanged", count: 5 })).toBe("5 task status changes");
    expect(describeCount({ kind: "SomethingNew", count: 2 })).toBe("2 × something new");
    expect(humanizeKind("BuildSucceeded")).toBe("build succeeded");
    expect(summaryLines(summary, 2)).toEqual(["5 task status changes", "1 approval request", "+2 more events"]);
  });

  it("writes headlines", () => {
    expect(headline(summary, "Payroll")).toBe("8 events in Payroll since your last session");
    expect(headline({ ...summary, counts: [], totalEvents: 0 }, null)).toBe("All quiet since your last session");
  });
});

describe("settings", () => {
  afterEach(() => cleanup());

  it("orders sections stably", () => {
    const icon = settingsSections[0]!.icon;
    const mk = (id: string, order: number): SettingsSection => ({ id, title: id, icon, order, component: () => null });
    const list = [mk("general", 0), mk("system", 900), mk("about", 1000), mk("a", 10), mk("b", 10)];
    expect(orderedSections(list).map((s) => s.id)).toEqual(["general", "a", "b", "system", "about"]);
    // Every area's section is registered and General stays first.
    const ids = orderedSections().map((s) => s.id);
    expect(ids[0]).toBe("general");
    expect(ids).toEqual(expect.arrayContaining(["providers", "manager", "system", "about"]));
  });

  it("formats sizes and durations", () => {
    // Units are separated by non-breaking spaces.
    const sp = (s: string) => s.replace(/ /g, " ");
    expect(sp(formatBytes(0))).toBe("0 B");
    expect(sp(formatBytes(1536))).toBe("1.5 KB");
    expect(sp(formatBytes(16 * 1024 ** 3))).toBe("16 GB");
    expect(formatBytes(null)).toBe("—");
    expect(sp(formatDuration(65_000))).toBe("1 min 5 sec");
    expect(sp(formatDuration(3 * 3600_000 + 12 * 60_000))).toBe("3 hr 12 min");
  });

  it("switch and segmented are keyboard accessible", () => {
    const onSwitch = vi.fn();
    const onSeg = vi.fn();
    render(
      <>
        <Switch label="Close to tray" checked={false} onChange={onSwitch} />
        <Segmented
          label="Mode"
          value="a"
          onChange={onSeg}
          options={[
            { value: "a", label: "A" },
            { value: "b", label: "B", disabled: true },
            { value: "c", label: "C" },
          ]}
        />
      </>,
    );
    const sw = screen.getByRole("switch", { name: "Close to tray" });
    expect(sw.getAttribute("aria-checked")).toBe("false");
    fireEvent.click(sw);
    expect(onSwitch).toHaveBeenCalledWith(true);
    fireEvent.keyDown(screen.getByRole("radiogroup", { name: "Mode" }), { key: "ArrowRight" });
    expect(onSeg).toHaveBeenCalledWith("c"); // skips the disabled option
  });

  it("general section renders settings and saves toggles", () => {
    const update = vi.fn(async () => true);
    useSettingsStore.setState({ settings: DEFAULT_SETTINGS, autostartSupported: true, loading: false, error: null, update });
    render(<GeneralSection projectId={null} />);
    fireEvent.click(screen.getByRole("switch", { name: "Close to tray" }));
    expect(update).toHaveBeenCalledWith({ closeToTray: false });
    fireEvent.click(screen.getByRole("radio", { name: "Forever" }));
    expect(update).toHaveBeenCalledWith({ eventRetentionDays: 0 });
  });

  it("general section shows a retry state without settings", () => {
    const load = vi.fn(async () => {});
    useSettingsStore.setState({ settings: null, loading: false, error: "boom", load });
    render(<GeneralSection projectId={null} />);
    expect(screen.getByText("boom")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /retry/i }));
    expect(load).toHaveBeenCalled();
  });
});
