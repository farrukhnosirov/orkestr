import type { AgentTemplate, Event, TeamLeadRun, TeamView } from "@orkestr/protocol";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { applyLanguagePreference } from "../../i18n";
import { useAppStore } from "../../store/app";
import {
  DEFAULT_POLICIES,
  applyTeamEvent,
  blankTeamDraft,
  hierarchyTree,
  memberAvatars,
  parseBudget,
  teamDescription,
  teamName,
  validateTeamDraft,
} from "./teamsModel";

const PROJECT = "0190f0b0-0000-7000-8000-ffffffffffff";
const TEAM = "0190f0b0-0000-7000-8000-00000000aaaa";
const LEAD_TPL = "0190f0b0-0000-7000-8000-000000000101";
const BE_TPL = "0190f0b0-0000-7000-8000-000000000102";
const RV_TPL = "0190f0b0-0000-7000-8000-000000000103";

function tpl(id: string, name: string, role: AgentTemplate["role"]): AgentTemplate {
  return { id, name, role } as AgentTemplate;
}
const TEMPLATES = [tpl(LEAD_TPL, "Software Architect", "Architect"), tpl(BE_TPL, "Backend Engineer", "Backend"), tpl(RV_TPL, "Code Reviewer", "Reviewer")];

const LEAD_RUN: TeamLeadRun = {
  id: "lead-1",
  teamId: TEAM,
  projectId: PROJECT,
  runId: null,
  parentTaskId: "0190f0b0-0000-7000-8000-000000000201",
  leadAgentId: "0190f0b0-0000-7000-8000-000000000301",
  title: "Build the payroll feature",
  status: "Executing",
  statusReason: null,
  depth: 1,
  subtasks: [
    {
      taskId: "0190f0b0-0000-7000-8000-000000000202",
      key: "s1",
      title: "Implement: payroll",
      role: "Backend",
      templateId: BE_TPL,
      status: "Running",
      agentId: "0190f0b0-0000-7000-8000-000000000302",
      attempts: 2,
      isReview: false,
      note: null,
    },
    {
      taskId: "0190f0b0-0000-7000-8000-000000000203",
      key: "review",
      title: "Review: payroll",
      role: "Reviewer",
      templateId: RV_TPL,
      status: "Planned",
      agentId: null,
      attempts: 0,
      isReview: true,
      note: null,
    },
  ],
  budgetUsd: null,
  costUsd: 0,
  summary: null,
  warnings: [],
  model: null,
  createdAt: "2026-10-05T10:00:00Z",
  updatedAt: "2026-10-05T10:00:00Z",
  finishedAt: null,
};

function view(patch: Partial<TeamView["team"]> = {}, rest: Partial<TeamView> = {}): TeamView {
  return {
    team: {
      id: TEAM,
      name: "ERP Team",
      kind: "Erp",
      description:
        "Business modules (accounting, payroll, inventory): an architect lead splits each feature for backend and database engineers; QA tests, a reviewer checks.",
      members: [
        { templateId: LEAD_TPL, isLead: true, count: 1, model: null },
        { templateId: BE_TPL, isLead: false, count: 2, model: null },
        { templateId: RV_TPL, isLead: false, count: 1, model: null },
      ],
      projectId: null,
      createdAt: "2026-10-05T09:00:00Z",
      policies: { ...DEFAULT_POLICIES, hierarchy: true },
      builtin: true,
      updatedAt: null,
      ...patch,
    },
    assignments: [],
    leadRuns: [LEAD_RUN],
    ...rest,
  };
}

function ev(kind: string, payload: unknown): Event {
  return { seq: 1, id: "e", ts: "", projectId: PROJECT, taskId: null, agentId: null, nodeId: null, correlationId: null, kind, payload } as Event;
}

const roleOf = (id: string) => TEMPLATES.find((x) => x.id === id)?.role ?? null;

describe("teams model", () => {
  it("validates drafts like the core", () => {
    const d = blankTeamDraft(TEMPLATES);
    expect(validateTeamDraft(d, roleOf)).toMatch(/name/i);
    d.name = "Payments";
    expect(validateTeamDraft(d, roleOf)).toBeNull();
    expect(validateTeamDraft({ ...d, members: [] }, roleOf)).toMatch(/members/);
    const twoLeads = { ...d, members: [{ templateId: LEAD_TPL, isLead: true, count: 1, model: null }, { templateId: BE_TPL, isLead: true, count: 1, model: null }] };
    expect(validateTeamDraft(twoLeads, roleOf)).toMatch(/one lead/);
    const hier = { ...d, policies: { ...d.policies, hierarchy: true } };
    expect(validateTeamDraft(hier, roleOf)).toMatch(/hierarchy/i);
    const onlyReviewer = { ...hier, members: [{ templateId: LEAD_TPL, isLead: true, count: 1, model: null }, { templateId: RV_TPL, isLead: false, count: 1, model: null }] };
    expect(validateTeamDraft(onlyReviewer, roleOf)).toMatch(/executor/);
    expect(validateTeamDraft({ ...d, policies: { ...d.policies, maxFanOut: 50 } }, roleOf)).toMatch(/1–12/);
    expect(validateTeamDraft({ ...d, budgetText: "-1" }, roleOf)).toMatch(/budget/i);
    expect(parseBudget("2,5")).toBe(2.5);
    expect(parseBudget("")).toBeNull();
  });

  it("builds the hierarchy tree and avatars", () => {
    const tree = hierarchyTree(view(), "Architect");
    expect(tree.level).toBe("manager");
    expect(tree.children[0]?.level).toBe("lead");
    expect(tree.children[0]?.children.map((c) => c.level)).toEqual(["executor", "reviewer"]);
    const av = memberAvatars(view().team.members, TEMPLATES);
    expect(av[0]?.isLead).toBe(true);
    expect(av.find((a) => a.role === "Backend")?.count).toBe(2);
  });

  it("applies team events", () => {
    const list = [view()];
    const renamed = applyTeamEvent(list, ev("TeamUpdated", { team: view({ name: "ERP Squad" }) }));
    expect(renamed[0]?.team.name).toBe("ERP Squad");
    const lead = applyTeamEvent(list, ev("TeamLeadRunUpdated", { lead: { ...LEAD_RUN, status: "Completed" } }));
    expect(lead[0]?.leadRuns[0]?.status).toBe("Completed");
    expect(applyTeamEvent(list, ev("TeamDeleted", { teamId: TEAM }))).toEqual([]);
    expect(applyTeamEvent(list, ev("TaskCreated", {}))).toBe(list);
  });

  it("translates built-in team names only while unchanged", async () => {
    await act(async () => {
      await applyLanguagePreference("ru");
    });
    expect(teamName(view().team)).toBe("ERP-команда");
    expect(teamDescription(view().team)).toMatch(/^Бизнес-модули/);
    expect(teamName(view({ name: "My ERP" }).team)).toBe("My ERP");
    expect(teamName(view({ builtin: false }).team)).toBe("ERP Team");
    await act(async () => {
      await applyLanguagePreference("en");
    });
  });
});

let handler: ((ev: Event) => void) | null = null;
const calls: Array<{ method: string; params: unknown }> = [];
const fakeClient = {
  call: vi.fn(async (method: string, params: unknown) => {
    calls.push({ method, params });
    switch (method) {
      case "teams.list":
        return { teams: [view()] };
      case "teams.assign":
        return view({}, { assignments: [{ teamId: TEAM, projectId: PROJECT, planId: null, permissions: { secrets: false, denied: [] }, assignedAt: "" }] });
      case "teams.create":
        return view({ id: "new", name: "Payments", builtin: false, kind: { Custom: "Payments" } });
      default:
        throw new Error(`unexpected ${method}`);
    }
  }),
  subscribe: vi.fn((_topics: string[], h: (ev: Event) => void) => {
    handler = h;
    return () => {
      handler = null;
    };
  }),
};

vi.mock("../../core/client", () => ({ maybeClient: () => fakeClient, getClient: () => fakeClient }));
vi.mock("../../features/providers/store", () => ({ useProvidersLive: () => ({ providers: [] }) }));

const { TeamsTab } = await import("./TeamsTab");

describe("TeamsTab", () => {
  beforeEach(() => {
    calls.length = 0;
    useAppStore.getState().setConnection({ state: "open" });
  });
  afterEach(() => cleanup());

  it("shows team cards, assigns, shows the hierarchy and validates the editor", async () => {
    render(<TeamsTab projectId={PROJECT} templates={TEMPLATES} />);
    await screen.findByText("ERP Team");
    expect(screen.getByText("Built-in")).toBeTruthy();
    expect(screen.getByText("1 lead working")).toBeTruthy();
    expect(screen.getByText("4 agents")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Assign to this project" }));
    await waitFor(() => expect(calls.some((c) => c.method === "teams.assign")).toBe(true));
    expect(calls.find((c) => c.method === "teams.assign")?.params).toEqual({ teamId: TEAM, projectId: PROJECT });

    // Live lead update + hierarchy tree.
    act(() => handler?.(ev("TeamLeadRunUpdated", { lead: { ...LEAD_RUN, title: "Payroll v2" } })));
    fireEvent.click(screen.getByRole("button", { name: "Show the hierarchy" }));
    const tree = await screen.findByRole("tree", { name: "Team hierarchy" });
    expect(tree.textContent).toContain("Payroll v2");
    expect(tree.textContent).toContain("Implement: payroll");
    expect(tree.textContent).toContain("attempt 2");

    // Editor: empty name is refused locally; a valid team is created.
    fireEvent.click(screen.getByRole("button", { name: "New team" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.click(screen.getByRole("button", { name: "Save team" }));
    expect((await screen.findByRole("alert")).textContent).toMatch(/name/i);
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "Payments" } });
    fireEvent.click(screen.getByRole("button", { name: "Save team" }));
    await waitFor(() => expect(calls.some((c) => c.method === "teams.create")).toBe(true));
    expect(dialog.isConnected).toBe(false);
  });
});

describe("TeamsTab in Russian", () => {
  beforeEach(async () => {
    useAppStore.getState().setConnection({ state: "open" });
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

  it("renders cards with Russian names and plurals", async () => {
    render(<TeamsTab projectId={PROJECT} templates={TEMPLATES} />);
    await screen.findByText("ERP-команда");
    expect(screen.getByText("4 агента")).toBeTruthy();
    expect(screen.getByText("работает 1 лид")).toBeTruthy();
    expect(screen.getByText("до 4 подзадач на лида")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Назначить этому проекту" })).toBeTruthy();
  });
});
