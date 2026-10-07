// Audit r2: the title bar has `backdrop-filter`, so it is a stacking context and
// the `z-index` of its popovers (notifications, layout switcher) only ranks them
// inside the bar. With the bar at z-index auto, the dock groups that follow it
// in the DOM (also stacking contexts) painted over — and swallowed clicks of —
// both popovers. jsdom has no layout, so this pins the CSS contract instead.
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

const css = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "app.css"), "utf8");

function rule(selector: string): Record<string, string> {
  const esc = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const m = new RegExp(`(?:^|\\n)${esc}\\s*\\{([^}]*)\\}`).exec(css);
  if (!m) throw new Error(`rule ${selector} not found`);
  const body = m[1]!.replace(/\/\*[\s\S]*?\*\//g, "");
  const out: Record<string, string> = {};
  for (const decl of body.split(";")) {
    const i = decl.indexOf(":");
    if (i > 0) out[decl.slice(0, i).trim()] = decl.slice(i + 1).trim();
  }
  return out;
}

describe("shell stacking order", () => {
  it("the title bar is a positioned layer above the workspace", () => {
    const bar = rule(".titlebar");
    const body = rule(".app__body");
    expect(bar["position"]).toBe("relative");
    const barZ = Number(bar["z-index"]);
    const bodyZ = Number(body["z-index"]);
    expect(Number.isFinite(barZ)).toBe(true);
    expect(Number.isFinite(bodyZ)).toBe(true);
    expect(barZ).toBeGreaterThan(bodyZ);
  });

  it("the workspace is its own stacking context (dockview z-indexes stay inside it)", () => {
    const body = rule(".app__body");
    expect(body["position"]).toBe("relative");
    expect(body["isolation"]).toBe("isolate");
  });
});
