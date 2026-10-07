// Audit r2: the per-project AI privacy policy and data classification (§56)
// can be set from Settings → Project privacy and are saved with `projects.update`.
import type { Project, ProviderView } from "@orkestr/protocol";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const calls: { method: string; params: unknown }[] = [];
let current: Project;
vi.mock("../../core/client", () => ({
  getClient: () => ({
    call: (method: string, params: unknown) => {
      calls.push({ method, params });
      if (method === "projects.update") {
        const { patch } = params as { patch: Partial<Project> };
        current = { ...current, ...patch };
        return Promise.resolve({ project: current });
      }
      return Promise.reject(new Error(`unexpected ${method}`));
    },
  }),
  maybeClient: () => null,
}));

import { i18n } from "../../i18n";
import { useAppStore } from "../../store/app";
import { useProvidersStore } from "../providers/store";
import { effectiveAiPolicy, policyForMode, toggleProvider } from "./privacy";
import { PrivacySettings } from "./PrivacySettings";

const project = (): Project =>
  ({
    id: "p1",
    name: "Payroll",
    slug: "payroll",
    description: "",
    rootPath: null,
    repositoryId: null,
    privacy: { ai: { mode: "CloudAllowed" }, sync: "Allowed" },
    classification: "Internal",
    approvalMode: "Ask",
    budget: null,
    color: null,
    icon: null,
    tags: [],
    archived: false,
    createdAt: "2026-01-01T00:00:00Z",
    updatedAt: "2026-01-01T00:00:00Z",
    lastOpenedAt: null,
  }) as Project;

const provider = (id: string, name: string, isLocal: boolean): ProviderView =>
  ({ provider: { id, kind: "OpenAiCompatible", name, isLocal } }) as unknown as ProviderView;

beforeEach(async () => {
  await i18n.changeLanguage("en");
  calls.length = 0;
  current = project();
  useAppStore.setState({ projects: [current], activeProjectId: "p1" });
  useProvidersStore.setState({
    providers: [provider("cloud", "Cloud API", false), provider("local", "Ollama box", true)],
    loaded: true,
    loading: false,
  });
});

afterEach(async () => {
  cleanup();
  await i18n.changeLanguage("en");
});

describe("privacy logic", () => {
  it("classification tightens the policy like the core", () => {
    expect(effectiveAiPolicy("Internal", { mode: "CloudAllowed" })).toEqual({ mode: "CloudAllowed" });
    expect(effectiveAiPolicy("Confidential", { mode: "CloudAllowed" })).toEqual({ mode: "LocalModelsOnly" });
    const picked = { mode: "SelectedProvidersOnly" as const, providers: ["a"] };
    expect(effectiveAiPolicy("Confidential", picked)).toEqual(picked);
    expect(effectiveAiPolicy("Secret", picked)).toEqual({ mode: "LocalModelsOnly" });
  });

  it("keeps the provider list when switching modes and toggles providers", () => {
    const picked = { mode: "SelectedProvidersOnly" as const, providers: ["a"] };
    expect(policyForMode("SelectedProvidersOnly", picked)).toEqual(picked);
    expect(policyForMode("SelectedProvidersOnly", { mode: "CloudAllowed" })).toEqual({ mode: "SelectedProvidersOnly", providers: [] });
    expect(policyForMode("LocalModelsOnly", picked)).toEqual({ mode: "LocalModelsOnly" });
    expect(toggleProvider(picked, "b", true)).toEqual({ mode: "SelectedProvidersOnly", providers: ["a", "b"] });
    expect(toggleProvider(picked, "a", false)).toEqual({ mode: "SelectedProvidersOnly", providers: [] });
  });
});

describe("Settings → Project privacy", () => {
  it("sets Local models only and a Secret classification via projects.update", async () => {
    render(<PrivacySettings projectId="p1" />);
    await act(async () => {
      fireEvent.click(screen.getByRole("radio", { name: "Local models only" }));
    });
    expect(calls[0]).toEqual({
      method: "projects.update",
      params: { projectId: "p1", patch: { privacy: { ai: { mode: "LocalModelsOnly" }, sync: "Allowed" } } },
    });
    await act(async () => {
      fireEvent.click(screen.getByRole("radio", { name: "Secret" }));
    });
    expect(calls[1]).toEqual({ method: "projects.update", params: { projectId: "p1", patch: { classification: "Secret" } } });
    expect(useAppStore.getState().projects[0]?.classification).toBe("Secret");
  });

  it("selects providers and explains a classification override", async () => {
    current = { ...project(), classification: "Confidential" };
    useAppStore.setState({ projects: [current] });
    render(<PrivacySettings projectId="p1" />);
    expect(screen.getByRole("status").textContent).toContain("Local models only");
    await act(async () => {
      fireEvent.click(screen.getByRole("radio", { name: "Selected providers only" }));
    });
    expect(screen.queryByRole("status")).toBeNull();
    await act(async () => {
      fireEvent.click(screen.getByRole("switch", { name: "Ollama box" }));
    });
    expect(calls.at(-1)).toEqual({
      method: "projects.update",
      params: { projectId: "p1", patch: { privacy: { ai: { mode: "SelectedProvidersOnly", providers: ["local"] }, sync: "Allowed" } } },
    });
  });

  it("asks to open a project and has Russian strings", async () => {
    const { unmount } = render(<PrivacySettings projectId={null} />);
    expect(screen.getByText("Open a project to set its AI privacy.")).toBeTruthy();
    unmount();
    await i18n.changeLanguage("ru");
    render(<PrivacySettings projectId="p1" />);
    expect(screen.getByRole("radio", { name: "Только локальные модели" })).toBeTruthy();
    expect(screen.getByRole("radio", { name: "Секретные" })).toBeTruthy();
  });
});
