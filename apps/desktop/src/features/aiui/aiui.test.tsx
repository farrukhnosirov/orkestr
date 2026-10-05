import type { Event, PreviewUiPatchResult, UiConfig, UiPatchOp } from "@orkestr/protocol";
import { UI_CONFIG_SCHEMA_VERSION } from "@orkestr/protocol";
import { defaultTheme } from "@orkestr/ui";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { i18n } from "../../i18n";
import { changeText, idLabel } from "./changeText";
import { checkPatch, isPathAllowed, isSafeText, parsePointer, summarizeChanges } from "./patch";
import { DEFAULT_PREFS, applyPrefsToRoot, isPanelHidden, isWidgetHidden, panelSide, prefsOf, themeOverlayVars } from "./prefs";
import { EMPTY_UNDO, canRedo, canUndo, completeRedo, completeUndo, MAX_UNDO, recordApplied, type UndoEntry } from "./undo";

// ── fake core ────────────────────────────────────────────

const BASE: UiConfig = {
  schemaVersion: UI_CONFIG_SCHEMA_VERSION,
  theme: { ...defaultTheme(), id: "nebula", name: "Nebula", mode: "Dark", tokens: {}, effects: { blurPx: 18, glowIntensity: 0.6, panelOpacity: 0.72, animations: true } },
  layout: { preset: "Coding", docking: null },
  performanceMode: "Balanced",
};
const PATCHED: UiConfig = {
  ...BASE,
  theme: { ...BASE.theme, tokens: { "color-bg": "#020308" } },
  prefs: { ...DEFAULT_PREFS, density: "Compact", backgroundDim: 0.55, hiddenWidgets: ["missionControl.tokenChart"], panels: { "missionControl.claude": { side: "Right", hidden: false } } },
};
const EXAMPLE: UiPatchOp[] = [
  { op: "replace", path: "/prefs/density", value: "Compact" },
  { op: "add", path: "/prefs/hiddenWidgets/-", value: "missionControl.tokenChart" },
  { op: "add", path: "/prefs/panels/missionControl.claude", value: { side: "Right", hidden: false } },
  { op: "replace", path: "/prefs/backgroundDim", value: 0.55 },
  { op: "add", path: "/theme/tokens/color-bg", value: "#020308" },
];
const CHANGES: PreviewUiPatchResult["changes"] = [
  { path: "/prefs/density", before: "Comfortable", after: "Compact" },
  { path: "/prefs/hiddenWidgets", before: [], after: ["missionControl.tokenChart"] },
  { path: "/prefs/panels/missionControl.claude", after: { side: "Right", hidden: false } },
  { path: "/prefs/backgroundDim", before: 0, after: 0.55 },
  { path: "/theme/tokens/color-bg", after: "#020308" },
];
const INVERSE: UiPatchOp[] = [{ op: "replace", path: "/prefs/density", value: "Comfortable" }];

let stored: { config: UiConfig; version: number } = { config: BASE, version: 3 };
const calls: { method: string; params: unknown }[] = [];
let handlers: ((ev: Event) => void)[] = [];
const fakeClient = {
  state: "open",
  call: vi.fn(async (method: string, params: { patch?: UiPatchOp[]; baseVersion?: number }) => {
    calls.push({ method, params });
    if (method === "ui.previewPatch") {
      const empty = (params.patch ?? []).length === 0;
      return { base: stored.config, version: stored.version, config: empty ? stored.config : PATCHED, changes: empty ? [] : CHANGES, inverse: empty ? [] : INVERSE, warnings: [] };
    }
    if (method === "ui.applyPatch") {
      const undoing = params.patch === INVERSE;
      stored = { config: undoing ? BASE : PATCHED, version: stored.version + 1 };
      return { config: stored.config, version: stored.version, changes: CHANGES, inverse: undoing ? EXAMPLE : INVERSE, warnings: [] };
    }
    throw new Error(`unexpected ${method}`);
  }),
  subscribe: vi.fn((_topics: string[], h: (ev: Event) => void) => {
    handlers.push(h);
    return () => {
      handlers = handlers.filter((x) => x !== h);
    };
  }),
};

vi.mock("../../core/client", () => ({
  maybeClient: () => fakeClient,
  getClient: () => fakeClient,
}));

const { useAiUiStore, effectiveConfig } = await import("./aiuiStore");
const { UiPatchCard } = await import("./UiPatchCard");

// ── pure logic ───────────────────────────────────────────

describe("UI patch validation (client mirror)", () => {
  it("parses JSON pointers", () => {
    expect(parsePointer("/prefs/panels/a~1b~0c")).toEqual(["prefs", "panels", "a/b~c"]);
    expect(parsePointer("")).toEqual([]);
    expect(parsePointer("prefs")).toBeNull();
    expect(parsePointer("/a~2")).toBeNull();
  });

  it("rejects unknown and forbidden paths", () => {
    const seg = (p: string) => parsePointer(p) ?? [];
    expect(isPathAllowed("Manager", seg("/prefs/density"))).toBe(true);
    expect(isPathAllowed("Manager", seg("/theme/tokens/color-bg"))).toBe(true);
    expect(isPathAllowed("Manager", seg("/schemaVersion"))).toBe(false);
    expect(isPathAllowed("Manager", seg("/layout/docking"))).toBe(false);
    expect(isPathAllowed("Manager", seg("/theme/tokens"))).toBe(false);
    expect(isPathAllowed("Manager", seg("/theme/tokens/a/b"))).toBe(false);
    expect(isPathAllowed("Manager", seg("/plugins/x"))).toBe(false);
    expect(isPathAllowed("Agent", seg("/__proto__/polluted"))).toBe(false);
    expect(isPathAllowed("User", seg("/layout/docking"))).toBe(true);
    expect(isPathAllowed("User", seg("/schemaVersion"))).toBe(false);
    expect(checkPatch([{ op: "replace", path: "", value: {} }], "User").map((i) => i.code)).toContain("forbiddenPath");
  });

  it("rejects code injection attempts", () => {
    for (const bad of [
      "url(https://evil.example/x.png)",
      "red; background: url(x)",
      "<script>alert(1)</script>",
      "javascript:alert(1)",
      "expression(alert(1))",
      "a{b}",
      "@import 'x'",
      "data:text/html;base64,AAAA",
      "`rm -rf /`",
      "eval(1)",
      "x".repeat(600),
    ]) {
      expect(isSafeText(bad), bad).toBe(false);
      expect(checkPatch([{ op: "add", path: "/theme/tokens/color-bg", value: bad }], "Manager").map((i) => i.code)).toContain("unsafeText");
    }
    // Nested values and member names are checked too.
    expect(checkPatch([{ op: "add", path: "/prefs/panels/x", value: { side: "Right", "<b>": true } }], "Manager")).toHaveLength(1);
    expect(checkPatch([{ op: "add", path: "/prefs/hiddenWidgets", value: ["ok", "</style>"] }], "Manager")).toHaveLength(1);
    expect(isSafeText("#020308")).toBe(true);
    expect(isSafeText("rgba(0, 0, 0, 0.5)")).toBe(true);
  });

  it("checks structure", () => {
    expect(checkPatch([], "Manager").map((i) => i.code)).toEqual(["empty"]);
    expect(checkPatch([{ op: "add", path: "/prefs/density" }], "Manager").map((i) => i.code)).toEqual(["missingValue"]);
    expect(checkPatch([{ op: "remove", path: "/prefs/panels/x" }], "Manager")).toEqual([]);
    const many = Array.from({ length: 65 }, () => ({ op: "remove" as const, path: "/prefs/panels/x" }));
    expect(checkPatch(many, "User").map((i) => i.code)).toContain("tooMany");
    expect(checkPatch(EXAMPLE, "Manager")).toEqual([]);
  });

  it("summarizes the Mission Control example in both languages", async () => {
    const summary = summarizeChanges(CHANGES);
    expect(summary.map((c) => c.kind)).toEqual(["density", "widgetsHidden", "panel", "backgroundDim", "token"]);
    await i18n.changeLanguage("en");
    expect(summary.map(changeText)).toEqual([
      "Density: compact",
      "Hide widget Token chart",
      "Claude lane: move to the right",
      "Darker background (55% dimming)",
      "Theme color «color-bg»",
    ]);
    await i18n.changeLanguage("ru");
    expect(changeText(summary[2]!)).toBe("Дорожка Claude: переместить вправо");
    expect(changeText({ kind: "widgetsHidden", ids: ["a.b", "c.d", "e.f"] })).toMatch(/^Скрыть 3 виджета/);
    expect(idLabel("missionControl.someNewThing")).toBe("«some new thing»");
    await i18n.changeLanguage("en");
  });

  it("expands whole-container changes", () => {
    const s = summarizeChanges([{ path: "/prefs", after: { ...DEFAULT_PREFS, density: "Compact", backgroundDim: 0.3 } }]);
    // `before` missing: every member that differs from "nothing" is listed; density and dim are among them.
    expect(s.some((c) => c.kind === "density")).toBe(true);
    expect(s.some((c) => c.kind === "backgroundDim")).toBe(true);
    expect(summarizeChanges([{ path: "/weird/path", after: 1 }])).toEqual([{ kind: "other", path: "/weird/path" }]);
  });
});

describe("undo stack", () => {
  const entry = (id: string): UndoEntry => ({ id, summary: id, author: "Manager", forward: EXAMPLE, inverse: INVERSE, at: 0 });

  it("records, undoes and redoes", () => {
    let s = recordApplied(EMPTY_UNDO, entry("a"));
    s = recordApplied(s, entry("b"));
    expect(canUndo(s)).toBe(true);
    expect(canRedo(s)).toBe(false);
    s = completeUndo(s, [{ op: "replace", path: "/prefs/density", value: "Compact" }]);
    expect(s.past.map((e) => e.id)).toEqual(["a"]);
    expect(s.future.map((e) => e.id)).toEqual(["b"]);
    expect(s.future[0]?.forward).toEqual([{ op: "replace", path: "/prefs/density", value: "Compact" }]);
    s = completeRedo(s, INVERSE);
    expect(s.past.map((e) => e.id)).toEqual(["a", "b"]);
    expect(canRedo(s)).toBe(false);
  });

  it("a new change clears redo; no-op changes are not recorded; size is capped", () => {
    let s = recordApplied(EMPTY_UNDO, entry("a"));
    s = completeUndo(s, EXAMPLE);
    s = recordApplied(s, entry("c"));
    expect(s.future).toEqual([]);
    expect(recordApplied(s, { ...entry("noop"), inverse: [] })).toBe(s);
    for (let i = 0; i < MAX_UNDO + 10; i += 1) s = recordApplied(s, entry(`e${i}`));
    expect(s.past.length).toBe(MAX_UNDO);
    expect(completeUndo(EMPTY_UNDO, [])).toBe(EMPTY_UNDO);
  });
});

describe("preferences → DOM", () => {
  it("applies density, dim and safe theme overrides", () => {
    const prefs = prefsOf(PATCHED);
    const root = document.createElement("div");
    applyPrefsToRoot(prefs, root);
    expect(root.dataset["okDensity"]).toBe("compact");
    expect(root.style.getPropertyValue("--ok-bg-dim")).toBe("0.55");
    expect(isWidgetHidden(prefs, "missionControl.tokenChart")).toBe(true);
    expect(panelSide(prefs, "missionControl.claude")).toBe("Right");
    expect(isPanelHidden(prefs, "missionControl.claude")).toBe(false);
    expect(prefsOf(BASE)).toBe(DEFAULT_PREFS);
    const vars = themeOverlayVars({ ...PATCHED, theme: { ...PATCHED.theme, tokens: { "color-bg": "#020308", "color-accent": "url(x)", "Bad Name": "#fff" }, effects: { ...BASE.theme.effects, blurPx: 99 } } });
    expect(vars).toEqual({ "--ok-color-bg": "#020308", "--ok-blur": "64px" });
    expect(themeOverlayVars(BASE)).toEqual({});
  });
});

// ── store + card ─────────────────────────────────────────

describe("proposal flow", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    calls.length = 0;
    stored = { config: BASE, version: 3 };
    useAiUiStore.setState({ config: null, version: 0, proposal: null, previewVisible: true, undo: EMPTY_UNDO, busy: false, error: null });
  });
  afterEach(() => cleanup());

  it("previews, keeps and undoes a Manager proposal", async () => {
    render(<UiPatchCard />);
    await act(() =>
      useAiUiStore.getState().receiveProposal({ proposalId: "p1", projectId: null, author: "Manager", summary: "Compact Mission Control", patch: EXAMPLE }),
    );
    expect(calls[0]).toEqual({ method: "ui.previewPatch", params: { patch: EXAMPLE, author: "Manager" } });
    // Live preview: the effective document is the patched one.
    expect(effectiveConfig(useAiUiStore.getState())).toBe(PATCHED);
    expect(screen.getByRole("dialog", { name: "UI change proposed" })).toBeTruthy();
    expect(screen.getByText("Compact Mission Control")).toBeTruthy();
    expect(screen.getByText("Hide widget Token chart")).toBeTruthy();

    // Compare shows the original.
    fireEvent.click(screen.getByRole("button", { name: "Show original" }));
    expect(effectiveConfig(useAiUiStore.getState())).toBe(BASE);
    fireEvent.click(screen.getByRole("button", { name: "Show original" }));

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Keep" }));
    });
    const apply = calls.find((c) => c.method === "ui.applyPatch");
    expect(apply?.params).toMatchObject({ patch: EXAMPLE, baseVersion: 3, author: "Manager", proposalId: "p1" });
    expect(useAiUiStore.getState()).toMatchObject({ config: PATCHED, version: 4 });
    expect(screen.getByText("Interface updated")).toBeTruthy();
    expect(canUndo(useAiUiStore.getState().undo)).toBe(true);

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Undo" }));
    });
    expect(useAiUiStore.getState().config).toBe(BASE);
    expect(canRedo(useAiUiStore.getState().undo)).toBe(true);
    await act(() => useAiUiStore.getState().redoLast());
    expect(useAiUiStore.getState().config).toBe(PATCHED);
  });

  it("revert drops the preview without storing anything", async () => {
    render(<UiPatchCard />);
    await act(() => useAiUiStore.getState().receiveProposal({ proposalId: "p2", projectId: null, author: "Manager", summary: "x", patch: EXAMPLE }));
    fireEvent.click(screen.getByRole("button", { name: "Revert" }));
    expect(useAiUiStore.getState().proposal).toBeNull();
    expect(calls.some((c) => c.method === "ui.applyPatch")).toBe(false);
    expect(effectiveConfig(useAiUiStore.getState())).toBe(BASE);
  });

  it("refuses unsafe proposals before calling the core", async () => {
    render(<UiPatchCard />);
    await act(() =>
      useAiUiStore.getState().receiveProposal({
        proposalId: "p3",
        projectId: null,
        author: "Manager",
        summary: "evil",
        patch: [{ op: "add", path: "/theme/tokens/color-bg", value: "url(javascript:alert(1))" }],
      }),
    );
    expect(calls).toEqual([]);
    expect(screen.getByText("The change was rejected")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Keep" })).toBeNull();
  });
});
