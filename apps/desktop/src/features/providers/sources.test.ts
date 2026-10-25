import type { AiSource, ProviderView, RoutingConfig } from "@orkestr/protocol";
import { describe, expect, it } from "vitest";

import { reorder, sourceStatus, toggleCapability, withSources } from "./sources";

const now = Date.parse("2026-10-26T12:00:00Z");
const later = "2026-10-26T13:30:00Z";
const src = (id: string, enabled = true): AiSource => ({ providerId: id, enabled, model: null, capabilities: null });

function view(status: Partial<ProviderView["status"]>, enabled = true): ProviderView {
  return {
    provider: { id: "a", name: "Claude Code", enabled },
    status: {
      health: "Healthy",
      authState: "Authenticated",
      rateLimitedUntil: null,
      quotaExhaustedUntil: null,
      cooldownUntil: null,
      ...status,
    },
  } as unknown as ProviderView;
}

describe("source status chips", () => {
  it("maps provider state to working / limit / sign-in / offline / cooldown", () => {
    expect(sourceStatus(view({}), src("a"), now).kind).toBe("ok");
    expect(sourceStatus(view({ quotaExhaustedUntil: later }), src("a"), now)).toEqual({ kind: "limit", until: later });
    expect(sourceStatus(view({ quotaExhaustedUntil: "2026-10-26T11:00:00Z" }), src("a"), now).kind).toBe("ok");
    expect(sourceStatus(view({ authState: "Invalid" }), src("a"), now).kind).toBe("auth");
    expect(sourceStatus(view({ health: "Down" }), src("a"), now).kind).toBe("offline");
    expect(sourceStatus(view({ cooldownUntil: later, health: "Degraded" }), src("a"), now).kind).toBe("cooldown");
    expect(sourceStatus(view({}), src("a", false), now).kind).toBe("disabled");
    expect(sourceStatus(undefined, src("a"), now).kind).toBe("unknown");
  });
});

describe("source list edits", () => {
  it("reorders, toggles capabilities and writes overrides in global order", () => {
    expect(reorder(["a", "b", "c"], 2, 0)).toEqual(["c", "a", "b"]);
    expect(reorder(["a", "b"], 0, 5)).toEqual(["a", "b"]);
    const s = toggleCapability(src("a"), "vision");
    expect(s.capabilities?.vision).toBe(false);
    const cfg = { mode: "Manual", rules: [] } as unknown as RoutingConfig;
    const out = withSources(cfg, [src("b"), src("a")], { voice: ["a", "b"], jarvis: undefined }, false);
    expect(out.sourcePriority.map((x) => x.providerId)).toEqual(["b", "a"]);
    expect(out.purposeSources).toEqual([{ purpose: "voice", sources: ["b", "a"] }]);
    expect(out.failover).toBe(false);
  });
});
