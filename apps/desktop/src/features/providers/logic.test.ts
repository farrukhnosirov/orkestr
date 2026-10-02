import type { ProviderKindInfo, ProviderStatus, ProviderView } from "@orkestr/protocol";
import { describe, expect, it } from "vitest";

import { commands } from "../commands/registry";
import { settingsSections } from "../settings/sections";
import {
  formatContext,
  formatLatency,
  formatPricing,
  formatUsd,
  groupByKind,
  headersFromRows,
  moveItem,
  parseOptionalNumber,
  parseRefKey,
  refKey,
  refLabel,
  statusOf,
  validateBaseUrl,
  validateManagerConfig,
} from "./logic";

const baseStatus: ProviderStatus = {
  health: "Unknown",
  authState: "Unverified",
  authDetail: null,
  healthDetail: null,
  latencyMs: null,
  avgLatencyMs: null,
  version: null,
  checkedAt: null,
  circuitOpen: false,
  rateLimitedUntil: null,
  quotaExhaustedUntil: null,
};

function view(id: string, kind: ProviderView["provider"]["kind"], status: Partial<ProviderStatus> = {}, enabled = true): ProviderView {
  return {
    provider: {
      id,
      kind,
      name: `${kind} ${id}`,
      enabled,
      baseUrl: null,
      authMethod: "ApiKey",
      credentialRefId: null,
      secretRefId: null,
      isLocal: false,
      defaultModel: null,
      settings: {},
      createdAt: "2026-10-01T00:00:00Z",
      updatedAt: "2026-10-01T00:00:00Z",
    },
    status: { ...baseStatus, ...status },
    hasCredential: false,
    capabilities: { tools: true, vision: false, structuredOutput: true, streaming: true, reasoning: false, codingAgent: false },
    maxContextTokens: 128000,
    locality: "Cloud",
    costModel: "PerToken",
    codingAgent: false,
  };
}

describe("statusOf", () => {
  it("maps health/auth/limits to dots", () => {
    expect(statusOf(view("1", "OpenAi", { health: "Healthy", authState: "Authenticated" }))).toMatchObject({ dot: "online", label: "Connected" });
    expect(statusOf(view("1", "OpenAi", { health: "RateLimited" }))).toMatchObject({ dot: "warning", pulse: true });
    expect(statusOf(view("1", "OpenAi", { health: "Degraded", quotaExhaustedUntil: "2026-10-02T00:00:00Z" })).label).toBe("Quota exhausted");
    expect(statusOf(view("1", "OpenAi", { authState: "Invalid" })).dot).toBe("error");
    expect(statusOf(view("1", "ClaudeCode", { authState: "Unconfigured", authDetail: "not installed (`claude` not found on PATH)" })).label).toBe("Not installed");
    expect(statusOf(view("1", "OpenAi", { health: "Healthy", circuitOpen: true })).label).toBe("Circuit open");
    expect(statusOf(view("1", "OpenAi", { health: "Healthy" }, false)).label).toBe("Disabled");
    expect(statusOf(view("1", "OpenAi")).dot).toBe("connecting");
  });
});

describe("formatters", () => {
  it("formats latency, usd, pricing and context", () => {
    expect(formatLatency(null)).toBe("—");
    expect(formatLatency(320.4)).toBe("320 ms");
    expect(formatLatency(1450)).toBe("1.4 s");
    expect(formatUsd(0.00123)).toBe("$0.0012");
    expect(formatUsd(2.5)).toBe("$2.50");
    expect(formatPricing({ inputPerMtokUsd: 3, outputPerMtokUsd: 15 })).toBe("$3 / $15 per 1M (est.)");
    expect(formatPricing({ inputPerMtokUsd: 0.15, outputPerMtokUsd: 0.6 })).toBe("$0.15 / $0.6 per 1M (est.)");
    expect(formatPricing(null)).toBe("no list price");
    expect(formatContext(128000)).toBe("128K");
    expect(formatContext(1048576)).toBe("1M");
  });
});

describe("fallback chain helpers", () => {
  it("moves items safely", () => {
    expect(moveItem(["a", "b", "c"], 2, 0)).toEqual(["c", "a", "b"]);
    expect(moveItem(["a", "b", "c"], 0, 1)).toEqual(["b", "a", "c"]);
    expect(moveItem(["a", "b"], 0, 5)).toEqual(["a", "b"]);
  });
  it("round-trips ref keys", () => {
    const ref = { providerId: "p1", model: "gpt::odd" };
    expect(parseRefKey(refKey(ref))).toEqual(ref);
    expect(parseRefKey("nope")).toBeNull();
    expect(refLabel(ref, [])).toBe("Unknown provider · gpt::odd");
  });
});

describe("validation", () => {
  it("rejects credential headers and bad urls", () => {
    expect(headersFromRows([{ name: "X-Team", value: "a" }, { name: "", value: "" }])).toEqual({ headers: { "X-Team": "a" } });
    expect("error" in headersFromRows([{ name: "Authorization", value: "Bearer x" }])).toBe(true);
    expect("error" in headersFromRows([{ name: "bad name", value: "x" }])).toBe(true);
    expect("error" in headersFromRows([{ name: "X-A", value: "a\nInjected: 1" }])).toBe(true);
    expect(validateBaseUrl("http://localhost:1234/v1")).toBeNull();
    expect(validateBaseUrl("")).toBeNull();
    expect(validateBaseUrl("localhost:1234")).not.toBeNull();
  });
  it("validates manager config like the core", () => {
    const ok = { model: null, maxParallelAgents: 4, budgetUsd: null, timeLimitMins: null, fallbackChain: [] };
    expect(validateManagerConfig(ok)).toBeNull();
    expect(validateManagerConfig({ ...ok, maxParallelAgents: 0 })).not.toBeNull();
    expect(validateManagerConfig({ ...ok, budgetUsd: -1 })).not.toBeNull();
    expect(validateManagerConfig({ ...ok, timeLimitMins: 2000 })).not.toBeNull();
    expect(validateManagerConfig({ ...ok, fallbackChain: [{ providerId: "p", model: " " }] })).not.toBeNull();
    expect(parseOptionalNumber("")).toBeNull();
    expect(parseOptionalNumber("2.5")).toBe(2.5);
    expect(Number.isNaN(parseOptionalNumber("abc"))).toBe(true);
  });
});

describe("grouping and registration", () => {
  it("groups instances under their kind cards", () => {
    const kinds = [{ kind: "OpenAi" }, { kind: "Mock" }] as ProviderKindInfo[];
    const groups = groupByKind(kinds, [view("1", "Mock"), view("2", "Mock"), view("3", "Anthropic")]);
    expect(groups.map((g) => [g.info.kind, g.instances.length])).toEqual([
      ["OpenAi", 0],
      ["Mock", 2],
    ]);
  });
  it("registers settings sections and palette commands", () => {
    expect(settingsSections.map((s) => s.id)).toEqual(expect.arrayContaining(["providers", "manager"]));
    expect(settingsSections.find((s) => s.id === "providers")?.title).toBe("AI Providers");
    expect(commands.some((c) => c.id === "providers.open")).toBe(true);
  });
});
