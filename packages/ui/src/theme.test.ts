// @vitest-environment jsdom
import { describe, expect, it } from "vitest";

import {
  BUILTIN_THEMES,
  ThemeValidationError,
  applyPerformanceMode,
  applyTheme,
  defaultUiConfig,
  isValidTokenValue,
  validateTheme,
  validateUiConfig,
} from "./theme";

describe("theme engine", () => {
  it("accepts the built-in themes and default config", () => {
    for (const t of Object.values(BUILTIN_THEMES)) expect(validateTheme(t)).toEqual([]);
    expect(validateUiConfig(defaultUiConfig()).ok).toBe(true);
  });

  it("rejects unsafe token values (same rules as Rust)", () => {
    for (const ok of [
      "#0a0f1e",
      "rgba(10, 20, 30, 0.5)",
      "12px",
      "'Inter', system-ui, sans-serif",
      "var(--ok-color-accent)",
      "linear-gradient(135deg, #22d3ee 0%, #8b5cf6 100%)",
    ]) {
      expect(isValidTokenValue(ok), ok).toBe(true);
    }
    for (const bad of ["url(http://x)", "red; background: url(x)", "expression(alert(1))", "<script>", "a{b}", "", "JavaScript:alert(1)"]) {
      expect(isValidTokenValue(bad), bad).toBe(false);
    }
  });

  it("reports every issue", () => {
    const cfg = defaultUiConfig();
    const bad = {
      ...cfg,
      schemaVersion: 99,
      theme: {
        ...cfg.theme,
        tokens: { "Bad Name": "#fff", "color-bg": "url(x)" },
        effects: { ...cfg.theme.effects, glowIntensity: 3 },
      },
    };
    const res = validateUiConfig(bad);
    expect(res.ok).toBe(false);
    if (!res.ok) expect(res.issues).toHaveLength(4);
  });

  it("applies tokens as CSS variables and cleans up on switch", () => {
    const el = document.createElement("div");
    const eclipse = BUILTIN_THEMES["eclipse"];
    const nebula = BUILTIN_THEMES["nebula"];
    if (!eclipse || !nebula) throw new Error("missing themes");
    applyTheme(eclipse, el);
    expect(el.style.getPropertyValue("--ok-color-accent")).toBe("#f472b6");
    expect(el.style.getPropertyValue("--ok-blur")).toBe("22px");
    expect(el.dataset["okTheme"]).toBe("eclipse");
    applyTheme(nebula, el);
    expect(el.style.getPropertyValue("--ok-color-accent")).toBe("");
    expect(el.dataset["okMode"]).toBe("dark");
    applyPerformanceMode("BatterySaver", el);
    expect(el.dataset["okPerformance"]).toBe("BatterySaver");
  });

  it("throws on invalid themes without touching the element", () => {
    const el = document.createElement("div");
    const nebula = BUILTIN_THEMES["nebula"];
    if (!nebula) throw new Error("missing theme");
    expect(() => applyTheme({ ...nebula, tokens: { "color-bg": "url(evil)" } }, el)).toThrow(ThemeValidationError);
    expect(el.style.length).toBe(0);
  });
});
