/** Quality pipeline / Self-Healing configuration is reachable from the UI (pipeline.getConfig / setConfig). */
import type { PipelineConfig } from "@orkestr/protocol";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const PROJECT = "0190f0b0-0000-7000-8000-ffffffffffff";

function defaults(): PipelineConfig {
  return {
    projectId: PROJECT,
    enabled: true,
    stages: ["Author", "Review", "Build", "Test", "Security", "Merge"],
    overrides: [],
    healing: { maxAttempts: 3, maxWallMins: 60, stopOnRepeat: true, escalateModel: true },
    merge: "PerPolicy",
    customized: false,
  };
}

let stored = defaults();
const sets: Record<string, unknown>[] = [];
const fakeClient = {
  state: "open",
  call: vi.fn(async (method: string, params: Record<string, unknown>) => {
    if (method === "pipeline.getConfig") return stored;
    if (method === "pipeline.setConfig") {
      sets.push(params);
      const { projectId: _p, reset, ...patch } = params;
      stored = reset ? defaults() : ({ ...stored, ...patch, customized: true } as PipelineConfig);
      return stored;
    }
    throw new Error(method);
  }),
};

vi.mock("../../core/client", () => ({ maybeClient: () => fakeClient, getClient: () => fakeClient }));

const { applyLanguagePreference } = await import("../../i18n");
const { PipelineSettings } = await import("./PipelineSettings");
const { addOverride, parseHealing, toggleStage } = await import("./configLogic");

describe("pipeline config logic", () => {
  it("keeps stages in pipeline order with Author always on", () => {
    expect(toggleStage(["Author", "Review", "Build"], "Security", true)).toEqual(["Author", "Review", "Build", "Security"]);
    expect(toggleStage(["Review", "Build"], "Review", false)).toEqual(["Author", "Build"]);
  });

  it("validates the healing limits like the core", () => {
    const base = { maxAttempts: "3", maxWallMins: "60", maxBudgetUsd: "", stopOnRepeat: true, escalateModel: false };
    expect(parseHealing(base)).toEqual({ limits: { maxAttempts: 3, maxWallMins: 60, stopOnRepeat: true, escalateModel: false } });
    expect(parseHealing({ ...base, maxAttempts: "11" })).toEqual({ error: "attempts" });
    expect(parseHealing({ ...base, maxAttempts: "1.5" })).toEqual({ error: "attempts" });
    expect(parseHealing({ ...base, maxWallMins: "0" })).toEqual({ error: "wallMins" });
    expect(parseHealing({ ...base, maxWallMins: "1441" })).toEqual({ error: "wallMins" });
    expect(parseHealing({ ...base, maxBudgetUsd: "-1" })).toEqual({ error: "budget" });
    expect(parseHealing({ ...base, maxBudgetUsd: "2,5" })).toMatchObject({ limits: { maxBudgetUsd: 2.5 } });
  });

  it("adds task-kind overrides once per kind", () => {
    const r = addOverride([], " Docs ", ["Author", "Review"]);
    expect(r).toEqual({ overrides: [{ kind: "Docs", stages: ["Author", "Review"] }] });
    expect(addOverride([{ kind: "docs", stages: [] }], "Docs", [])).toEqual({ error: "duplicate" });
    expect(addOverride([], "  ", [])).toEqual({ error: "kindRequired" });
  });
});

describe("PipelineSettings", () => {
  beforeEach(async () => {
    stored = defaults();
    sets.length = 0;
    await applyLanguagePreference("en");
  });
  afterEach(() => cleanup());

  it("turns a stage off, lowers self-healing and disables the pipeline", async () => {
    render(<PipelineSettings projectId={PROJECT} />);
    const security = await screen.findByRole("switch", { name: "Security stage" });
    await act(async () => fireEvent.click(security));
    expect(sets.at(-1)).toEqual({ projectId: PROJECT, stages: ["Author", "Review", "Build", "Test", "Merge"] });

    fireEvent.change(screen.getByRole("textbox", { name: "Fix attempts" }), { target: { value: "1" } });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Save limits" })));
    expect(sets.at(-1)).toEqual({ projectId: PROJECT, healing: { maxAttempts: 1, maxWallMins: 60, stopOnRepeat: true, escalateModel: true } });

    await act(async () => fireEvent.click(screen.getByRole("switch", { name: "Run the quality pipeline" })));
    expect(sets.at(-1)).toEqual({ projectId: PROJECT, enabled: false });
    await waitFor(() => expect(screen.getByText("Customized")).toBeTruthy());
  });

  it("rejects an invalid limit without calling the core", async () => {
    render(<PipelineSettings projectId={PROJECT} />);
    fireEvent.change(await screen.findByRole("textbox", { name: "Time limit, min" }), { target: { value: "0" } });
    fireEvent.click(screen.getByRole("button", { name: "Save limits" }));
    expect(screen.getByRole("alert").textContent).toContain("1 to 1440");
    expect(sets).toEqual([]);
  });

  it("is translated in Russian", async () => {
    await applyLanguagePreference("ru");
    render(<PipelineSettings projectId={PROJECT} />);
    expect(await screen.findByRole("switch", { name: "Запускать конвейер качества" })).toBeTruthy();
    expect(screen.getByText("Ограничения самовосстановления")).toBeTruthy();
  });
});
