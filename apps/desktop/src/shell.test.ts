import type { Event, Project } from "@orkestr/protocol";
import { describe, expect, it } from "vitest";

import { displayOrder, fuzzyScore, type PaletteItem } from "./components/CommandPalette";
import { projectHue, projectInitials } from "./components/ProjectRail";
import { describeEvent, formatTime, severityOf } from "./panels/Logs/describe";
import { CODING_PRESET, getPanel, PANELS } from "./panels/registry";
import { sortProjects, useAppStore } from "./store/app";

function project(name: string, createdAt: string, lastOpenedAt: string | null = null): Project {
  return {
    id: `0190f0b0-0000-7000-8000-${String(name.length).padStart(12, "0")}${name}`.slice(0, 36),
    name,
    slug: name.toLowerCase(),
    description: "",
    rootPath: null,
    repositoryId: null,
    privacy: { ai: { mode: "CloudAllowed" }, sync: "Allowed" },
    classification: "Internal",
    approvalMode: "Ask",
    budget: null,
    color: null,
    icon: null,
    tags: [],
    archived: false,
    createdAt,
    updatedAt: createdAt,
    lastOpenedAt,
  };
}

describe("panel registry", () => {
  it("contains every §14 panel exactly once", () => {
    const expected = [
      "Manager", "Code", "Files", "Terminal", "Git", "Agents", "Tasks", "Architecture",
      "Browser", "Database", "Logs", "Deployments", "Memory", "Costs", "Monitoring",
      // mission-control (Phase 3, §32)
      "Mission Control",
      // embedded VS Code (VSCodium REH web)
      "VS Code",
      // VS Code extensions for the native editor (ADR-0025)
      "Extensions",
      // Claude Code chat (official CLI)
      "Claude Code",
      // multi-node (Phase 6)
      "Nodes",
    ];
    expect(PANELS.map((p) => p.title).sort()).toEqual([...expected].sort());
    expect(new Set(PANELS.map((p) => p.id)).size).toBe(PANELS.length);
  });

  it("preset only references registered panels, anchors placed earlier", () => {
    const placed = new Set<string>();
    for (const slot of CODING_PRESET) {
      expect(getPanel(slot.panel)).toBeDefined();
      if (slot.position) expect(placed.has(slot.position.referencePanel)).toBe(true);
      placed.add(slot.panel);
    }
  });
});

describe("store", () => {
  it("keeps a stable rail order and activates the most recently opened project", () => {
    const a = project("Alpha", "2026-01-01T00:00:00Z");
    const b = project("Beta", "2026-01-02T00:00:00Z", "2026-02-01T00:00:00Z");
    const c = project("Charlie", "2026-01-03T00:00:00Z");
    expect(sortProjects([c, b, a]).map((p) => p.name)).toEqual(["Alpha", "Beta", "Charlie"]);
    useAppStore.getState().setActiveProject(null);
    useAppStore.getState().setProjects([a, b, c]);
    expect(useAppStore.getState().activeProjectId).toBe(b.id);
    useAppStore.getState().removeProject(b.id);
    expect(useAppStore.getState().activeProjectId).toBe(a.id);
  });
});

describe("helpers", () => {
  it("project initials and hue", () => {
    expect(projectInitials("Payroll Platform")).toBe("PP");
    expect(projectInitials("schoolify")).toBe("SC");
    expect(projectInitials("  ")).toBe("?");
    expect(projectHue("abc")).toBe(projectHue("abc"));
  });

  it("fuzzy scoring prefers substrings and rejects non-matches", () => {
    expect(fuzzyScore("crea", "Create Project…")).toBeGreaterThan(fuzzyScore("cpj", "Create Project…"));
    expect(fuzzyScore("cpj", "Create Project…")).toBeGreaterThan(0);
    expect(fuzzyScore("zzz", "Create Project…")).toBe(-1);
  });

  it("describes events", () => {
    const ev: Event = {
      seq: 3,
      id: "e3",
      ts: "2026-10-02T10:11:12.345Z",
      projectId: null,
      taskId: null,
      agentId: null,
      nodeId: null,
      correlationId: null,
      kind: "Log",
      payload: { level: "error", target: "orkestr_core", message: "boom", fields: null },
    };
    expect(describeEvent(ev)).toBe("boom");
    expect(severityOf(ev)).toBe("error");
    expect(formatTime(ev.ts)).toMatch(/^\d\d:\d\d:\d\d\.345$/);
  });
});

describe("command palette", () => {
  it("selects in display (grouped) order, not ranking order", () => {
    const item = (id: string, group: string): PaletteItem => ({ id, title: id, group, indices: [], run: () => undefined });
    // Ranked A(g1), B(g2), C(g1) is displayed as g1: A, C — g2: B; the second
    // row (index 1) must be C, the one the user sees highlighted.
    const ordered = displayOrder([item("A", "g1"), item("B", "g2"), item("C", "g1")]);
    expect(ordered.map((i) => i.id)).toEqual(["A", "C", "B"]);
  });
});
