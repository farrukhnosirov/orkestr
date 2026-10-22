// Alert rules editor and log sources manager (ADR-0042): typed forms per rule
// kind, client-side validation, built-in sources read-only, and the pure
// helpers. The core client is mocked.
import type { AlertRule, LogSourceInfo } from "@orkestr/protocol";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const calls: { method: string; params: Record<string, unknown> }[] = [];
let rules: AlertRule[] = [];
let sources: LogSourceInfo[] = [];

const fake = {
  call: vi.fn((method: string, params: Record<string, unknown>) => {
    calls.push({ method, params });
    switch (method) {
      case "monitor.rules":
        return Promise.resolve({ rules });
      case "monitor.ruleUpsert": {
        const rule = params.rule as AlertRule;
        rules = [...rules.filter((r) => r.id !== rule.id), rule];
        return Promise.resolve({ rule });
      }
      case "monitor.ruleDelete":
        rules = rules.filter((r) => r.id !== params.ruleId);
        return Promise.resolve({});
      case "monitor.checksList":
        return Promise.resolve({ checks: [] });
      case "monitor.logSources":
        return Promise.resolve({ sources });
      case "monitor.sourceAdd": {
        const source: LogSourceInfo = { id: "file:1", name: (params.name as string) ?? (params.path as string), kind: { type: "File", path: params.path as string } };
        sources = [...sources, source];
        return Promise.resolve({ source });
      }
      case "monitor.sourceRemove":
        sources = sources.filter((s) => s.id !== params.sourceId);
        return Promise.resolve({});
      default:
        return Promise.reject(new Error(`unexpected ${method}`));
    }
  }),
  subscribe: vi.fn(() => () => {}),
};

vi.mock("../../../core/client", () => ({ getClient: () => fake, maybeClient: () => fake }));

import { i18n } from "../../../i18n";
import { useAppStore } from "../../../store/app";
import { RulesEditor, SourcesManager } from "./rules";
import {
  draftFromRule,
  EMPTY_RULE,
  isAbsolutePath,
  isRemovableSource,
  ruleDraftToRule,
  slugId,
  sourceDraftToParams,
} from "./rulesModel";

const cpu: AlertRule = {
  id: "cpu-high",
  name: "CPU above 95%",
  severity: "Warning",
  enabled: true,
  kind: { type: "MetricThreshold", metric: "cpu", above: 95, samples: 12 },
};
const logs: AlertRule = {
  id: "log-errors",
  name: "Error burst in logs",
  severity: "Warning",
  enabled: true,
  kind: { type: "LogRate", minLevel: "error", count: 20, windowSecs: 60 },
};

beforeEach(async () => {
  calls.length = 0;
  rules = [cpu, logs];
  sources = [
    { id: "ingest:web", name: "web", kind: { type: "Ingest", stream: "web" } },
    { id: "file:abc", name: "App log", kind: { type: "File", path: "C:\\logs\\app.log" } },
  ];
  useAppStore.setState((s) => ({ connection: { ...s.connection, state: "open" } }));
  vi.spyOn(globalThis, "confirm").mockReturnValue(true);
  await i18n.changeLanguage("en");
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("RulesEditor", () => {
  it("lists rules with summaries and creates a log-rate rule through the typed form", async () => {
    render(<RulesEditor />);
    const row = await screen.findByTestId("rule-cpu-high");
    expect(within(row).getByText("CPU above 95%")).toBeTruthy();
    expect(within(row).getByText(/for 12 samples in a row/)).toBeTruthy();

    fireEvent.click(screen.getByText("New rule"));
    const form = screen.getByRole("form", { name: "New alert rule" });
    fireEvent.change(within(form).getByLabelText("Name"), { target: { value: "API timeouts" } });
    fireEvent.change(within(form).getByLabelText("Condition"), { target: { value: "LogRate" } });
    fireEvent.change(within(form).getByLabelText("Text contains"), { target: { value: "timeout" } });
    fireEvent.change(within(form).getByLabelText("Window, seconds"), { target: { value: "0" } });
    fireEvent.click(within(form).getByText("Save"));
    expect(await within(form).findByText("Enter a whole number of seconds from 1 to 86400.")).toBeTruthy();
    expect(calls.some((c) => c.method === "monitor.ruleUpsert")).toBe(false);

    fireEvent.change(within(form).getByLabelText("Window, seconds"), { target: { value: "120" } });
    fireEvent.click(within(form).getByText("Save"));
    await waitFor(() =>
      expect(calls.find((c) => c.method === "monitor.ruleUpsert")?.params).toEqual({
        rule: {
          id: "api-timeouts",
          name: "API timeouts",
          severity: "Warning",
          enabled: true,
          kind: { type: "LogRate", count: 10, windowSecs: 120, minLevel: "error", pattern: "timeout" },
        },
      }),
    );
    expect(await screen.findByTestId("rule-api-timeouts")).toBeTruthy();
  });

  it("edits, disables and deletes a rule", async () => {
    render(<RulesEditor />);
    const row = await screen.findByTestId("rule-cpu-high");
    fireEvent.click(within(row).getByRole("checkbox"));
    await waitFor(() =>
      expect(calls.find((c) => c.method === "monitor.ruleUpsert")?.params).toEqual({ rule: { ...cpu, enabled: false } }),
    );
    fireEvent.click(within(await screen.findByTestId("rule-cpu-high")).getByLabelText("Edit rule"));
    const form = screen.getByRole("form", { name: "Edit alert rule" });
    expect((within(form).getByLabelText("Id") as HTMLInputElement).disabled).toBe(true);
    fireEvent.change(within(form).getByLabelText("Threshold, percent"), { target: { value: "80" } });
    fireEvent.click(within(form).getByText("Save"));
    await waitFor(() => expect(rules.find((r) => r.id === "cpu-high")?.kind).toEqual({ type: "MetricThreshold", metric: "cpu", above: 80, samples: 12 }));

    fireEvent.click(within(await screen.findByTestId("rule-log-errors")).getByLabelText("Delete rule"));
    await waitFor(() => expect(calls.some((c) => c.method === "monitor.ruleDelete" && c.params.ruleId === "log-errors")).toBe(true));
    await waitFor(() => expect(screen.queryByTestId("rule-log-errors")).toBeNull());
  });
});

describe("SourcesManager", () => {
  it("shows built-in sources read-only, adds and removes a file source", async () => {
    render(<SourcesManager />);
    const ingest = await screen.findByTestId("source-ingest:web");
    expect(within(ingest).getByText("Built in")).toBeTruthy();
    expect(within(ingest).queryByLabelText("Remove source")).toBeNull();

    fireEvent.click(screen.getByText("Add log file"));
    const form = screen.getByRole("form", { name: "Tail a log file" });
    fireEvent.change(within(form).getByLabelText("File path"), { target: { value: "logs/app.log" } });
    fireEvent.click(within(form).getByText("Add source"));
    expect(await within(form).findByText("Enter an absolute file path.")).toBeTruthy();
    fireEvent.change(within(form).getByLabelText("File path"), { target: { value: "D:\\srv\\api.log" } });
    fireEvent.change(within(form).getByLabelText("Display name"), { target: { value: "API" } });
    fireEvent.click(within(form).getByText("Add source"));
    await waitFor(() =>
      expect(calls.find((c) => c.method === "monitor.sourceAdd")?.params).toEqual({ path: "D:\\srv\\api.log", name: "API" }),
    );
    expect(await screen.findByTestId("source-file:1")).toBeTruthy();

    fireEvent.click(within(screen.getByTestId("source-file:abc")).getByLabelText("Remove source"));
    await waitFor(() => expect(calls.some((c) => c.method === "monitor.sourceRemove" && c.params.sourceId === "file:abc")).toBe(true));
  });

  it("is localized in Russian", async () => {
    await i18n.changeLanguage("ru");
    render(<SourcesManager />);
    expect(await screen.findByText("Встроенный")).toBeTruthy();
    expect(screen.getByText("Добавить файл лога")).toBeTruthy();
  });
});

describe("rules model", () => {
  it("builds wire rules per kind and validates like the core", () => {
    expect(ruleDraftToRule(draftFromRule(cpu))).toEqual({ rule: cpu });
    expect(ruleDraftToRule(draftFromRule(logs))).toEqual({ rule: logs });
    const down = ruleDraftToRule({ ...EMPTY_RULE, name: "Any check", kind: "CheckDown" });
    expect(down).toEqual({ rule: { id: "any-check", name: "Any check", severity: "Warning", enabled: true, kind: { type: "CheckDown" } } });
    const base = { ...EMPTY_RULE, name: "x" };
    expect(ruleDraftToRule({ ...base, name: " " })).toEqual({ error: "name" });
    expect(ruleDraftToRule({ ...base, id: "bad id" })).toEqual({ error: "id" });
    expect(ruleDraftToRule({ ...base, metric: "net" })).toEqual({ error: "metric" });
    expect(ruleDraftToRule({ ...base, metric: "gpu1" })).toHaveProperty("rule");
    for (const above of ["", "100", "-1", "x"]) expect(ruleDraftToRule({ ...base, above })).toEqual({ error: "above" });
    for (const samples of ["0", "1001", "2.5"]) expect(ruleDraftToRule({ ...base, samples })).toEqual({ error: "samples" });
    const log = { ...base, kind: "LogRate" as const };
    expect(ruleDraftToRule({ ...log, count: "0" })).toEqual({ error: "count" });
    expect(ruleDraftToRule({ ...log, windowSecs: "86401" })).toEqual({ error: "window" });
    expect(ruleDraftToRule({ ...log, pattern: "x".repeat(201) })).toEqual({ error: "pattern" });
    expect(slugId("CPU hot!! (prod)")).toBe("cpu-hot-prod");
  });

  it("validates file sources and knows which sources are removable", () => {
    expect(isAbsolutePath("C:\\logs\\a.log")).toBe(true);
    expect(isAbsolutePath("/var/log/syslog")).toBe(true);
    expect(isAbsolutePath("\\\\server\\share\\a.log")).toBe(true);
    expect(isAbsolutePath("\\\\.\\NUL")).toBe(false);
    expect(isAbsolutePath("logs/a.log")).toBe(false);
    const existing: LogSourceInfo[] = [{ id: "file:1", name: "App", kind: { type: "File", path: "C:\\a.log" } }];
    expect(sourceDraftToParams("", "c:\\A.log", existing)).toEqual({ error: "duplicate" });
    expect(sourceDraftToParams("app", "C:\\b.log", existing)).toEqual({ error: "duplicate" });
    expect(sourceDraftToParams(" ", " C:\\b.log ", existing)).toEqual({ params: { path: "C:\\b.log" } });
    expect(sourceDraftToParams("", "", existing)).toEqual({ error: "path" });
    expect(isRemovableSource(existing[0]!)).toBe(true);
    expect(isRemovableSource({ id: "deploy:1", name: "prod", kind: { type: "Deployment", deploymentId: "1" } })).toBe(false);
  });
});
