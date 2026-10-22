// Bug Detective panel (Phase 7, ADR-0042): list + timeline + hypothesis +
// evidence + fix diff, the two gates and cancel, open from an alert and a
// manual report, and pure helpers. The core client is mocked.
import type { Alert, Incident } from "@orkestr/protocol";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const calls: { method: string; params: Record<string, unknown> }[] = [];
let incidents: Incident[] = [];
let settings = { autoOpenOnAlert: false, defaultBudgetUsd: 2, maxFixAttempts: 2 };

const alert: Alert = {
  id: "al-1234567890",
  projectId: "p1",
  rule: "log-errors",
  severity: "Critical",
  title: "Errors in api",
  message: "3 error lines",
  source: { type: "Log", sourceId: "api" },
  state: "Open",
  count: 1,
  openedAt: "2026-10-22T10:00:00Z",
};

function incident(over: Partial<Incident> = {}): Incident {
  return {
    id: "inc-1",
    projectId: "p1",
    title: "Divide by zero in api",
    trigger: { type: "Alert", alertId: "al-1234567890" },
    stage: "AwaitingFixApproval",
    evidence: [
      {
        id: "ev1",
        kind: "StackTrace",
        title: "Stack trace (1 frames)",
        content: "src/calc.rs:2:5",
        collectedAt: "2026-10-22T10:00:01Z",
      },
      {
        id: "ev2",
        kind: "CodeSnippet",
        title: "src/calc.rs:2",
        content: ">    2 |     a / b",
        reference: "src/calc.rs:2",
        collectedAt: "2026-10-22T10:00:01Z",
      },
    ],
    hypothesis: {
      summary: "div() divides by zero",
      rootCause: "the guard was removed",
      confidence: 0.82,
      evidenceRefs: ["ev2"],
      suspectedFiles: ["src/calc.rs:2"],
      proposedFix: "restore the guard",
      reproduction: "cargo test div",
    },
    reproduction: {
      command: "cargo test div",
      exitCode: 101,
      passed: false,
      outputTail: "panicked: attempt to divide by zero",
      finishedAt: "2026-10-22T10:00:05Z",
    },
    budgetUsd: 2,
    spentUsd: 0.05,
    createdAt: "2026-10-22T10:00:00Z",
    updatedAt: "2026-10-22T10:00:05Z",
    ...over,
  };
}

vi.mock("../../core/client", () => ({
  getClient: () => ({
    call: (method: string, params: Record<string, unknown>) => {
      calls.push({ method, params });
      switch (method) {
        case "detective.list":
          return Promise.resolve({ incidents });
        case "monitor.alerts":
          return Promise.resolve({ alerts: [alert] });
        case "detective.open":
          return Promise.resolve({
            incident: incident({ id: "inc-new", stage: "Opened" }),
          });
        case "detective.approveFix":
          incidents = [
            incident({ stage: "Fixing", fixBranch: "detective/inc-1" }),
          ];
          return Promise.resolve({ incident: incidents[0] });
        case "detective.approveMerge":
          incidents = [
            incident({ stage: "Resolved", fixBranch: "detective/inc-1" }),
          ];
          return Promise.resolve({ incident: incidents[0] });
        case "detective.cancel":
          incidents = [
            incident({ stage: "Cancelled", error: "cancelled by the user" }),
          ];
          return Promise.resolve({ incident: incidents[0] });
        case "detective.settings":
          return Promise.resolve(settings);
        case "detective.setSettings":
          settings = params as unknown as typeof settings;
          return Promise.resolve(settings);
        case "vcs.diff":
          return Promise.resolve({
            files: [
              {
                path: "src/calc.rs",
                status: "Modified",
                additions: 1,
                deletions: 0,
                binary: false,
                patch: "+    if b == 0 { return 0; }",
                truncated: false,
              },
            ],
            additions: 1,
            deletions: 0,
          });
        default:
          return Promise.reject(new Error(`unexpected ${method}`));
      }
    },
  }),
  maybeClient: () => null,
}));

import { i18n } from "../../i18n";
import BugDetectivePanel from "./index";
import {
  confidencePct,
  draftToSettings,
  inferStop,
  settingsToDraft,
  sortIncidents,
  timeline,
} from "./model";

beforeEach(async () => {
  calls.length = 0;
  incidents = [incident()];
  await i18n.changeLanguage("en");
});
afterEach(cleanup);

describe("BugDetective model", () => {
  it("builds the timeline for open, resolved and failed incidents", () => {
    const open = timeline(incident());
    expect(open.find((s) => s.state === "current")?.stage).toBe(
      "AwaitingFixApproval",
    );
    expect(open.filter((s) => s.state === "done")).toHaveLength(4);
    expect(
      timeline(incident({ stage: "Resolved" })).every(
        (s) => s.state === "done",
      ),
    ).toBe(true);
    const failed = timeline(
      incident({ stage: "Failed", fixBranch: "detective/x" }),
    );
    expect(failed.find((s) => s.state === "failed")?.stage).toBe("Fixing");
    expect(
      timeline(incident({ stage: "Cancelled" }), "Collecting").find(
        (s) => s.state === "cancelled",
      )?.stage,
    ).toBe("Collecting");
    expect(
      inferStop(
        incident({
          hypothesis: undefined,
          reproduction: undefined,
          evidence: [],
        }),
      ),
    ).toBe("Collecting");
  });

  it("clamps confidence and sorts open incidents first", () => {
    expect(confidencePct(1.4)).toBe(100);
    expect(confidencePct(Number.NaN)).toBe(0);
    const s = sortIncidents([
      incident({
        id: "a",
        stage: "Resolved",
        createdAt: "2026-10-23T00:00:00Z",
      }),
      incident({ id: "b", createdAt: "2026-10-21T00:00:00Z" }),
    ]);
    expect(s.map((i) => i.id)).toEqual(["b", "a"]);
  });
});

describe("BugDetective panel", () => {
  it("asks for a project without one", () => {
    render(<BugDetectivePanel projectId={null} />);
    expect(
      screen.getByText("Open a project to investigate its incidents."),
    ).toBeTruthy();
  });

  it("shows hypothesis, evidence and timeline; approves the fix", async () => {
    render(<BugDetectivePanel projectId="p1" />);
    const view = await screen.findByTestId("incident-view");
    expect(within(view).getByText("div() divides by zero")).toBeTruthy();
    expect(within(view).getByText("confidence 82%")).toBeTruthy();
    expect(within(view).getByText("Evidence (2 items)")).toBeTruthy();
    expect(
      within(view).getByText("Reproduction (expected to fail)"),
    ).toBeTruthy();
    fireEvent.click(within(view).getByRole("button", { name: "ev2" }));
    fireEvent.click(screen.getByTestId("approve-fix"));
    await waitFor(() =>
      expect(
        calls.some(
          (c) =>
            c.method === "detective.approveFix" &&
            c.params.incidentId === "inc-1",
        ),
      ).toBe(true),
    );
    await waitFor(() => expect(screen.getByTestId("fix-diff")).toBeTruthy());
  });

  it("merges only after a confirming second click and shows the fix diff", async () => {
    incidents = [
      incident({
        stage: "AwaitingMergeApproval",
        fixBranch: "detective/inc-1",
        fixTests: {
          command: "cargo test",
          passed: true,
          outputTail: "ok",
          finishedAt: "2026-10-22T10:01:00Z",
        },
      }),
    ];
    render(<BugDetectivePanel projectId="p1" />);
    await screen.findByText((text) => text.includes("if b == 0"));
    const diffCall = calls.find((c) => c.method === "vcs.diff");
    expect(diffCall?.params.target).toEqual({
      kind: "Range",
      from: "HEAD",
      to: "detective/inc-1",
    });
    fireEvent.click(screen.getByTestId("approve-merge"));
    expect(calls.some((c) => c.method === "detective.approveMerge")).toBe(
      false,
    );
    fireEvent.click(screen.getByTestId("confirm-merge"));
    await waitFor(() =>
      expect(calls.some((c) => c.method === "detective.approveMerge")).toBe(
        true,
      ),
    );
    await waitFor(() =>
      expect(screen.getByText("The fix has been merged.")).toBeTruthy(),
    );
  });

  it("cancels an incident", async () => {
    render(<BugDetectivePanel projectId="p1" />);
    fireEvent.click(await screen.findByTestId("cancel-incident"));
    await waitFor(() =>
      expect(screen.getByTestId("incident-error").textContent).toBe(
        "cancelled by the user",
      ),
    );
    expect(screen.queryByTestId("cancel-incident")).toBeNull();
  });

  it("opens incidents from an alert and from a manual report", async () => {
    incidents = [];
    render(<BugDetectivePanel projectId="p1" />);
    fireEvent.click(await screen.findByTestId(`investigate-${alert.id}`));
    await waitFor(() =>
      expect(
        calls.find((c) => c.method === "detective.open")?.params.trigger,
      ).toEqual({ type: "Alert", alertId: alert.id }),
    );
    fireEvent.click(screen.getByTestId("detective-report"));
    fireEvent.change(screen.getByTestId("report-text"), {
      target: { value: "Checkout returns 500" },
    });
    fireEvent.click(screen.getByTestId("report-submit"));
    await waitFor(() =>
      expect(
        calls.filter((c) => c.method === "detective.open")[1]?.params.trigger,
      ).toEqual({ type: "Manual", description: "Checkout returns 500" }),
    );
  });

  it("edits and saves the detective settings", async () => {
    settings = { autoOpenOnAlert: false, defaultBudgetUsd: 2, maxFixAttempts: 2 };
    render(<BugDetectivePanel projectId="p1" />);
    fireEvent.click(await screen.findByTestId("detective-settings-toggle"));
    const auto = (await screen.findByTestId("settings-auto-open")) as HTMLInputElement;
    expect(auto.checked).toBe(false);
    fireEvent.click(auto);
    fireEvent.change(screen.getByTestId("settings-budget"), { target: { value: "0" } });
    fireEvent.click(screen.getByTestId("settings-save"));
    expect(await screen.findByText("Enter an amount above 0 and up to 1000.")).toBeTruthy();
    expect(calls.some((c) => c.method === "detective.setSettings")).toBe(false);
    fireEvent.change(screen.getByTestId("settings-budget"), { target: { value: "7.5" } });
    fireEvent.change(screen.getByTestId("settings-attempts"), { target: { value: "4" } });
    fireEvent.click(screen.getByTestId("settings-save"));
    await waitFor(() =>
      expect(calls.find((c) => c.method === "detective.setSettings")?.params).toEqual({
        autoOpenOnAlert: true,
        defaultBudgetUsd: 7.5,
        maxFixAttempts: 4,
      }),
    );
    expect(await screen.findByText("Settings saved.")).toBeTruthy();
  });

  it("is localized in Russian", async () => {
    await i18n.changeLanguage("ru");
    render(<BugDetectivePanel projectId="p1" />);
    expect(await screen.findByText("Одобрить исправление")).toBeTruthy();
    expect(screen.getByText("Гипотеза о причине")).toBeTruthy();
  });
});

describe("detective settings helpers", () => {
  it("validates the budget and attempts like the core", () => {
    const d = settingsToDraft({ autoOpenOnAlert: true, defaultBudgetUsd: 2, maxFixAttempts: 2 });
    expect(draftToSettings(d)).toEqual({ settings: { autoOpenOnAlert: true, defaultBudgetUsd: 2, maxFixAttempts: 2 } });
    expect(draftToSettings({ ...d, defaultBudgetUsd: "1000" })).toHaveProperty("settings");
    expect(draftToSettings({ ...d, defaultBudgetUsd: "2,5" })).toEqual({ settings: { autoOpenOnAlert: true, defaultBudgetUsd: 2.5, maxFixAttempts: 2 } });
    for (const b of ["", "0", "-1", "1000.01", "abc"]) expect(draftToSettings({ ...d, defaultBudgetUsd: b })).toEqual({ error: "budget" });
    for (const n of [0, 6, 2.5]) expect(draftToSettings({ ...d, maxFixAttempts: n })).toEqual({ error: "attempts" });
  });
});
