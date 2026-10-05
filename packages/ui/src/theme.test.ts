// @vitest-environment jsdom
import type { ThemeConfig } from "@orkestr/protocol";
import { describe, expect, it } from "vitest";

import fixtures from "../../../crates/protocol/fixtures/ui_theme/cases.json";
import {
  BUILTIN_THEMES,
  BUILTIN_THEME_ORDER,
  THEME_APPLIED_EVENT,
  ThemeValidationError,
  applyPerformanceMode,
  applyTheme,
  contrastReport,
  defaultUiConfig,
  deriveAccentTokens,
  effectiveVisuals,
  exportThemeFile,
  formatColor,
  hsvaToRgba,
  isValidTokenValue,
  matchesKind,
  normalizeTheme,
  onThemeApplied,
  parseColor,
  parseThemeFile,
  rgbaToHsva,
  validateTheme,
  validateUiConfig,
} from "./theme";

function theme(id: string): ThemeConfig {
  const t = BUILTIN_THEMES[id];
  if (!t) throw new Error(`missing theme ${id}`);
  return structuredClone(t);
}

type Json = Record<string, unknown>;
function merge(dst: Json, patch: Json): Json {
  const out: Json = { ...dst };
  for (const [k, v] of Object.entries(patch)) {
    const cur = out[k];
    out[k] =
      v && typeof v === "object" && !Array.isArray(v) && cur && typeof cur === "object" && !Array.isArray(cur)
        ? merge(cur as Json, v as Json)
        : v;
  }
  return out;
}

describe("shared fixtures (identical rules to Rust)", () => {
  it("parses colors", () => {
    for (const [input, expected] of fixtures.colors as [string, number[] | null][]) {
      const got = parseColor(input);
      if (expected === null) {
        expect(got, input).toBeNull();
      } else {
        expect(got, input).not.toBeNull();
        const arr = [got?.r, got?.g, got?.b, got?.a] as number[];
        arr.forEach((v, i) => expect(Math.abs(v - (expected[i] ?? NaN)), `${input}[${i}]`).toBeLessThan(0.01));
      }
    }
  });

  it("matches token grammars", () => {
    for (const [kind, value, ok] of fixtures.kinds as [Parameters<typeof matchesKind>[0], string, boolean][]) {
      expect(matchesKind(kind, value), `${kind} ${value}`).toBe(ok);
    }
  });

  it("validates themes and computes contrast", () => {
    const base = theme("nebula") as unknown as Json;
    for (const c of fixtures.themes as { name: string; patch: Json; issues: string[]; contrast?: Record<string, number> }[]) {
      const t = normalizeTheme(merge(base, c.patch));
      const got = validateTheme(t)
        .map((i) => i.path)
        .sort();
      expect(got, c.name).toEqual([...c.issues].sort());
      const report = contrastReport(t);
      for (const [id, ratio] of Object.entries(c.contrast ?? {})) {
        const check = report.find((r) => r.id === id);
        expect(check, `${c.name} ${id}`).toBeDefined();
        expect(Math.abs((check?.ratio ?? 0) - ratio), `${c.name} ${id}`).toBeLessThan(0.011);
      }
    }
  });
});

describe("built-in themes", () => {
  it("are six valid themes with Nebula first and pass every contrast rule", () => {
    expect(BUILTIN_THEME_ORDER).toEqual(["nebula", "midnight-pro", "aurora", "carbon", "solar-light", "high-contrast"]);
    for (const id of BUILTIN_THEME_ORDER) {
      const t = theme(id);
      expect(validateTheme(t), id).toEqual([]);
      for (const c of contrastReport(t)) expect(c.pass, `${id} ${c.id} ${c.ratio}`).toBe(true);
    }
    expect(contrastReport(theme("high-contrast")).every((c) => c.ratio >= 7)).toBe(true);
    expect(theme("solar-light").mode).toBe("Light");
    expect(validateUiConfig(defaultUiConfig()).ok).toBe(true);
  });
});

describe("token safety", () => {
  it("rejects unsafe token values", () => {
    for (const ok of ["#0a0f1e", "rgba(10, 20, 30, 0.5)", "12px", "'Inter', system-ui, sans-serif", "var(--ok-color-accent)"]) {
      expect(isValidTokenValue(ok), ok).toBe(true);
    }
    for (const bad of ["url(http://x)", "red; background: url(x)", "expression(alert(1))", "<script>", "a{b}", "", "JavaScript:alert(1)"]) {
      expect(isValidTokenValue(bad), bad).toBe(false);
    }
  });

  it("reports every issue of a config", () => {
    const cfg = defaultUiConfig();
    const bad = {
      ...cfg,
      schemaVersion: 99,
      theme: { ...cfg.theme, tokens: { "Bad Name": "#fff", "color-bg": "url(x)" }, effects: { ...cfg.theme.effects, glowIntensity: 3 } },
    };
    const res = validateUiConfig(bad);
    expect(res.ok).toBe(false);
    if (!res.ok) expect(res.issues.map((i) => i.path).sort()).toEqual(["schemaVersion", "theme.effects.glowIntensity", "theme.tokens.Bad Name", "theme.tokens.color-bg"]);
  });
});

describe("theme files", () => {
  it("round-trips and accepts v1 files with section defaults", () => {
    const nebula = theme("nebula");
    const parsed = parseThemeFile(exportThemeFile(nebula));
    expect(parsed).toEqual({ ok: true, theme: nebula });
    const v1 = `{"format":"orkestr-theme","themeSchemaVersion":1,"theme":{"id":"old","name":"Old","mode":"Dark","tokens":{},"effects":{"blurPx":10,"glowIntensity":0.5,"panelOpacity":0.8,"animations":true}}}`;
    const old = parseThemeFile(v1);
    expect(old.ok).toBe(true);
    if (old.ok) expect(old.theme.background.kind).toBe("Nebula");
    expect(parseThemeFile(v1.replace("orkestr-theme", "x")).ok).toBe(false);
    expect(parseThemeFile(v1.replace('"themeSchemaVersion":1', '"themeSchemaVersion":9')).ok).toBe(false);
    expect(parseThemeFile("{nope").ok).toBe(false);
  });
});

describe("effective visuals per performance mode", () => {
  const nebula = theme("nebula");
  it("Ultra Visual uses WebGL at 60 fps", () => {
    const v = effectiveVisuals(nebula, { performanceMode: "UltraVisual", reducedMotion: false });
    expect(v.background).toMatchObject({ renderer: "webgl", animated: true, fps: 60, resolution: 1 });
    expect(v.blurPx).toBe(18);
    expect(v.motion).toBe("full");
  });
  it("Balanced uses a half-resolution canvas at 30 fps", () => {
    const v = effectiveVisuals(nebula, { performanceMode: "Balanced", reducedMotion: false });
    expect(v.background).toMatchObject({ renderer: "canvas", animated: true, fps: 30, resolution: 0.5 });
  });
  it("Performance freezes the background and drops blur/glow", () => {
    const v = effectiveVisuals(nebula, { performanceMode: "Performance", reducedMotion: false });
    expect(v.background).toMatchObject({ renderer: "canvas", animated: false, fps: 0 });
    expect([v.blurPx, v.glowIntensity, v.motion]).toEqual([0, 0, "reduced"]);
    expect(v.panelOpacity).toBeGreaterThanOrEqual(0.9);
  });
  it("Battery Saver is CSS only with no transitions", () => {
    const v = effectiveVisuals(nebula, { performanceMode: "BatterySaver", reducedMotion: false });
    expect(v.background.renderer).toBe("css");
    expect(v.durationScale).toBe(0);
  });
  it("reduced motion freezes the background in every mode", () => {
    const v = effectiveVisuals(nebula, { performanceMode: "UltraVisual", reducedMotion: true });
    expect(v.background.animated).toBe(false);
    expect(v.motionIntensity).toBe(0);
  });
  it("WebGL needs the theme flag and a nebula background", () => {
    const t = theme("nebula");
    t.background.webgl = false;
    expect(effectiveVisuals(t, { performanceMode: "UltraVisual", reducedMotion: false }).background.renderer).toBe("canvas");
  });
  it("motion speed scales durations", () => {
    const t = theme("nebula");
    t.motion.speed = 2;
    expect(effectiveVisuals(t).durationScale).toBe(0.5);
  });
});

describe("applyTheme", () => {
  it("applies tokens, effects and clean-up on switch, and notifies", () => {
    const el = document.createElement("div");
    const seen: string[] = [];
    const off = onThemeApplied((d) => seen.push(d.themeId), el);
    applyTheme(theme("solar-light"), el);
    expect(el.style.getPropertyValue("--ok-color-accent")).toBe("#b45309");
    expect(el.style.getPropertyValue("--ok-blur")).toBe("14px");
    expect(el.dataset["okMode"]).toBe("light");
    applyTheme(theme("nebula"), el);
    expect(el.style.getPropertyValue("--ok-color-accent")).toBe("");
    expect(el.dataset["okMode"]).toBe("dark");
    expect(el.style.getPropertyValue("--ok-duration-base")).toBe("200ms");
    applyTheme(theme("nebula"), el, { performanceMode: "BatterySaver", reducedMotion: false });
    expect(el.style.getPropertyValue("--ok-blur")).toBe("0px");
    expect(el.style.getPropertyValue("--ok-duration-base")).toBe("0ms");
    expect(el.dataset["okPerformance"]).toBe("BatterySaver");
    off();
    applyTheme(theme("carbon"), el);
    expect(seen).toEqual(["solar-light", "nebula", "nebula"]);
    expect(THEME_APPLIED_EVENT).toBe("ok-theme-applied");
    applyPerformanceMode("Performance", el);
    expect(el.dataset["okPerformance"]).toBe("Performance");
  });

  it("throws on invalid themes without touching the element", () => {
    const el = document.createElement("div");
    expect(() => applyTheme({ ...theme("nebula"), tokens: { "color-bg": "url(evil)" } }, el)).toThrow(ThemeValidationError);
    expect(() => applyTheme({ ...theme("nebula"), tokens: { "color-text": "#05060f" } }, el)).toThrow(ThemeValidationError);
    expect(el.style.length).toBe(0);
  });
});

describe("color math", () => {
  it("round-trips HSV", () => {
    for (const hex of ["#22d3ee", "#000000", "#ffffff", "#b45309", "#7c3aed"]) {
      const c = parseColor(hex);
      if (!c) throw new Error(hex);
      expect(formatColor(hsvaToRgba(rgbaToHsva(c)))).toBe(hex);
    }
    expect(formatColor({ r: 10, g: 20, b: 30, a: 0.5 })).toBe("rgba(10, 20, 30, 0.5)");
  });

  it("derives valid accent tokens", () => {
    const tokens = deriveAccentTokens("#7c3aed", "Dark");
    expect(Object.keys(tokens).sort()).toEqual(["color-accent", "color-accent-soft", "color-accent-strong", "color-border-accent"]);
    const t = { ...theme("nebula"), tokens };
    expect(validateTheme(t)).toEqual([]);
    expect(deriveAccentTokens("nope", "Dark")).toEqual({});
  });
});
