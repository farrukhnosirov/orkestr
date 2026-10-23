// Parallel engines: status tones of the title-bar indicator, roles, engine of a provider.
import type { EnginesConfig, EngineStatus } from "@orkestr/protocol";
import { describe, expect, it } from "vitest";

import { configChanged, engineReasonText, engineTone, hasRole, modeUses, providerEngine, roleKey, toggleRole } from "./logic";

function st(patch: Partial<EngineStatus> = {}): EngineStatus {
  return {
    engine: "codex",
    installed: true,
    signedIn: true,
    version: "0.163.0",
    plan: "plus",
    providerId: null,
    limitPercent: 10,
    limitResetsAt: null,
    runningAgents: 0,
    reason: null,
    ...patch,
  };
}

const cfg: EnginesConfig = {
  mode: "split",
  manager: "claudeCode",
  primary: "claudeCode",
  codexRoles: ["Backend", "QA"],
  claudeModel: "sonnet",
  codexModel: "default",
  crossFallback: true,
};

describe("engine tone", () => {
  it("green signed in / amber limit > 80 % / grey not installed / red not signed in", () => {
    expect(engineTone(st())).toBe("ok");
    expect(engineTone(st({ limitPercent: 80 }))).toBe("ok");
    expect(engineTone(st({ limitPercent: 81 }))).toBe("warn");
    expect(engineTone(st({ installed: false, signedIn: false }))).toBe("off");
    expect(engineTone(st({ signedIn: false, limitPercent: 99 }))).toBe("error");
    expect(engineTone(null)).toBe("off");
  });
});

describe("roles and config", () => {
  it("toggles roles regardless of spelling and maps role labels", () => {
    expect(hasRole(["Qa"], "QA")).toBe(true);
    expect(toggleRole(["Backend", "QA"], "qa")).toEqual(["Backend"]);
    expect(toggleRole(["Backend"], "UI/UX")).toEqual(["Backend", "UI/UX"]);
    expect(roleKey("UI/UX")).toBe("UiUx");
    expect(roleKey("Product Manager")).toBe("ProductManager");
    expect(roleKey("QA")).toBe("Qa");
    expect(roleKey("Backend")).toBe("Backend");
  });

  it("detects changes (role order does not matter)", () => {
    expect(configChanged(cfg, { ...cfg, codexRoles: ["QA", "Backend"] })).toBe(false);
    expect(configChanged(cfg, { ...cfg, mode: "balanced" })).toBe(true);
    expect(configChanged(cfg, { ...cfg, crossFallback: false })).toBe(true);
    expect(modeUses("single")).toEqual({ primary: true, roles: false });
    expect(modeUses("split")).toEqual({ primary: false, roles: true });
  });

  it("derives an agent's engine from its provider kind", () => {
    expect(providerEngine("ClaudeCode")).toBe("claudeCode");
    expect(providerEngine("Codex")).toBe("codex");
    expect(providerEngine("OpenAi")).toBe("api");
    expect(providerEngine(undefined)).toBeNull();
  });
});

describe("engineReasonText", () => {
  it("localizes the known not-installed reason and passes others through", () => {
    const t = (k: string) => `T:${k}`;
    expect(engineReasonText("Claude Code is not installed (install the Claude Code extension or the CLI)", t)).toBe("T:reason.claudeNotInstalled");
    expect(engineReasonText("Claude Code is not installed", t)).toBe("T:reason.claudeNotInstalled");
    expect(engineReasonText("something else", t)).toBe("something else");
  });
});
