/**
 * Guard for the webview entry (`main.tsx`): every window (main shell, HUD,
 * detached panel) must mount exactly one root. A merge once rendered `<App />`
 * twice, which stacked two shells in the main window (the page scrolled and
 * every panel, store subscription and robot host ran twice) and drew the full
 * shell behind the HUD and detached panels.
 */
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

const source = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "main.tsx"), "utf8");
const count = (needle: string) => source.split(needle).length - 1;

describe("webview entry", () => {
  it("mounts exactly one root component per window", () => {
    expect(count("<App />")).toBe(1);
    expect(count("<HudApp />")).toBe(1);
    expect(count("<DetachedPanelApp ")).toBe(1);
    expect(count(".render(")).toBe(1);
  });
});
