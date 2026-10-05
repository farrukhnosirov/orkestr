import type { ArchDiff, ArchEdge, ArchMapResult, ArchNode, ArchitectureMap, Event } from "@orkestr/protocol";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import { applyLanguagePreference, i18n } from "../../i18n";
import { useAppStore } from "../../store/app";
import { useEditorStore } from "../../store/editor";
import {
  assignColumns,
  diffMarks,
  exportSvg,
  highlightSet,
  layoutArchitecture,
  matchesQuery,
  nodeFiles,
  visibleGraph,
} from "./model";

const PROJECT = "0192a000-0000-7000-8000-0000000000aa";

function node(id: string, kind: ArchNode["kind"], name: string, extra: Partial<ArchNode> = {}): ArchNode {
  return {
    id,
    kind,
    name,
    ecosystem: kind === "Service" ? "compose" : kind === "Database" ? "compose" : "cargo",
    path: null,
    detail: null,
    files: [],
    fileCount: 0,
    tags: [],
    routes: [],
    dependencies: [],
    ...extra,
  };
}

function edge(from: string, to: string, kind: ArchEdge["kind"]): ArchEdge {
  return { id: `${from}->${to}:${kind}`, from, to, kind, weight: 1, labels: [] };
}

const NODES: ArchNode[] = [
  node("svc:api", "Service", "api-service"),
  node("mod:api", "Module", "api", {
    path: "crates/api",
    files: ["crates/api/Cargo.toml"],
    fileCount: 3,
    tags: ["binary"],
  }),
  node("mod:shared", "Module", "shared", { path: "crates/shared", files: ["crates/shared/Cargo.toml"] }),
  node("api:api", "Api", "api routes", {
    routes: [{ method: "GET", path: "/health", file: "crates/api/src/main.rs", line: 4, framework: "axum" }],
  }),
  node("db:postgres", "Database", "postgres", { detail: "postgres:16" }),
  node("ext:cargo", "External", "cargo packages", { dependencies: ["axum", "serde"] }),
];
const EDGES: ArchEdge[] = [
  edge("svc:api", "mod:api", "Runs"),
  edge("mod:api", "mod:shared", "DependsOn"),
  edge("mod:api", "api:api", "Exposes"),
  edge("mod:api", "db:postgres", "Uses"),
  edge("mod:shared", "ext:cargo", "External"),
];

function map(extra: Partial<ArchitectureMap> = {}): ArchitectureMap {
  return {
    snapshotId: "snap-2",
    projectId: PROJECT,
    generatedAt: new Date().toISOString(),
    digest: "d",
    nodes: NODES,
    edges: EDGES,
    stats: {
      modules: 2,
      services: 1,
      databases: 1,
      apis: 1,
      routes: 1,
      externalDependencies: 2,
      edges: EDGES.length,
      filesScanned: 12,
      filesParsed: 12,
      scanMs: 40,
    },
    warnings: [],
    truncated: false,
    ...extra,
  };
}

describe("architecture model", () => {
  it("puts services left, modules layered by dependency, APIs/DBs then externals right", () => {
    const col = assignColumns(NODES, EDGES);
    expect(col.get("svc:api")).toBe(0);
    expect(col.get("mod:api")).toBe(1);
    expect(col.get("mod:shared")).toBe(2);
    expect(col.get("api:api")).toBe(3);
    expect(col.get("db:postgres")).toBe(3);
    expect(col.get("ext:cargo")).toBe(4);
    const pos = layoutArchitecture(NODES, EDGES);
    expect(pos.size).toBe(NODES.length);
    expect(pos.get("svc:api")!.x).toBeLessThan(pos.get("mod:api")!.x);
    expect(pos.get("api:api")!.x).toBe(pos.get("db:postgres")!.x);
    expect(pos.get("api:api")!.y).not.toBe(pos.get("db:postgres")!.y);
  });

  it("survives import cycles", () => {
    const nodes = [node("a", "Module", "a"), node("b", "Module", "b")];
    const col = assignColumns(nodes, [edge("a", "b", "Imports"), edge("b", "a", "Imports")]);
    expect(col.size).toBe(2);
  });

  it("filters kinds, searches and highlights neighbours", () => {
    const g = visibleGraph({ nodes: NODES, edges: EDGES }, new Set(["External"]));
    expect(g.nodes.some((n) => n.kind === "External")).toBe(false);
    expect(g.edges.some((e) => e.kind === "External")).toBe(false);
    expect(matchesQuery(NODES[3]!, "/health")).toBe(true);
    expect(matchesQuery(NODES[5]!, "serde")).toBe(true);
    expect(matchesQuery(NODES[1]!, "nope")).toBe(false);
    expect(highlightSet(NODES, EDGES, "", null)).toBeNull();
    expect([...highlightSet(NODES, EDGES, "postgres", null)!]).toEqual(["db:postgres"]);
    expect([...highlightSet(NODES, EDGES, "", "mod:shared")!].sort()).toEqual(["ext:cargo", "mod:api", "mod:shared"]);
    expect(nodeFiles(NODES[3]!)).toEqual(["crates/api/src/main.rs"]);
  });

  it("marks diff nodes and exports escaped SVG", () => {
    const diff: ArchDiff = {
      fromSnapshotId: "snap-1",
      toSnapshotId: "snap-2",
      addedNodes: [NODES[4]!],
      removedNodes: [],
      changedNodes: [{ id: "api:api", kind: "Api", name: "api routes", fields: ["routes"], addedRoutes: [], removedRoutes: [], addedDependencies: [], removedDependencies: [] }],
      addedEdges: [],
      removedEdges: [],
    };
    const marks = diffMarks(diff);
    expect(marks.get("db:postgres")).toBe("added");
    expect(marks.get("api:api")).toBe("changed");
    const svg = exportSvg(
      [node("x", "Module", "<evil & co>")],
      [],
      { background: "#000", text: "#fff", muted: "#999", edge: "#555", kind: { Service: "#1", Module: "#2", Api: "#3", Database: "#4", External: "#5" } },
      (k) => k,
    );
    expect(svg.startsWith("<svg")).toBe(true);
    expect(svg).toContain("&lt;evil &amp; co&gt;");
    expect(svg).not.toContain("<evil");
  });
});

// ── Panel ───────────────────────────────────────────────

const calls: Array<{ method: string; params: unknown }> = [];
let handlers: Array<(ev: Event) => void> = [];
let mapResult: ArchMapResult = { map: map(), snapshots: [] };

const fakeClient = {
  call: vi.fn(async (method: string, params: unknown) => {
    calls.push({ method, params });
    switch (method) {
      case "architecture.map":
        return mapResult;
      case "architecture.refresh":
        return { map: mapResult.map, changed: false, diff: null };
      case "architecture.diff":
        return {
          diff: {
            fromSnapshotId: "snap-1",
            toSnapshotId: "snap-2",
            addedNodes: [NODES[4]],
            removedNodes: [node("mod:old", "Module", "legacy")],
            changedNodes: [],
            addedEdges: [],
            removedEdges: [],
          },
        };
      default:
        throw new Error(`unexpected ${method}`);
    }
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

const { default: ArchitecturePanel } = await import("./index");

beforeAll(() => {
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
});

describe("ArchitecturePanel", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    calls.length = 0;
    mapResult = {
      map: map(),
      snapshots: [
        { snapshotId: "snap-2", generatedAt: new Date().toISOString(), digest: "d", stats: map().stats },
        { snapshotId: "snap-1", generatedAt: "2026-01-01T10:00:00Z", digest: "c", stats: map().stats },
      ],
    };
    useAppStore.getState().setConnection({ state: "open" });
  });
  afterEach(async () => {
    cleanup();
    await act(async () => {
      await applyLanguagePreference("en");
    });
  });

  it("shows the empty state without a project", () => {
    render(<ArchitecturePanel projectId={null} />);
    expect(screen.getByText("No project open")).toBeTruthy();
  });

  it("lists nodes, filters kinds, opens details and files in Code", async () => {
    render(<ArchitecturePanel projectId={PROJECT} />);
    await screen.findByRole("region", { name: "Architecture map" });
    expect(calls[0]).toEqual({ method: "architecture.map", params: { projectId: PROJECT } });
    fireEvent.click(screen.getByRole("tab", { name: "List" }));
    const list = screen.getByRole("region", { name: "Architecture nodes by kind" });
    expect(within(list).getByText("api-service")).toBeTruthy();
    expect(within(list).getByText("postgres")).toBeTruthy();

    // Kind toggle hides databases.
    fireEvent.click(screen.getByRole("button", { name: "Databases: 1" }));
    expect(within(list).queryByText("postgres")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Databases: 1" }));

    // Search narrows the list.
    fireEvent.change(screen.getByLabelText("Search the map"), { target: { value: "health" } });
    expect(within(list).queryByText("shared")).toBeNull();
    expect(within(list).getByText("api routes")).toBeTruthy();
    fireEvent.change(screen.getByLabelText("Search the map"), { target: { value: "" } });

    // Details + open a route file in Code.
    fireEvent.click(within(list).getByText("api routes"));
    const detail = screen.getByRole("complementary", { name: "Details: api routes" });
    expect(within(detail).getByText("/health")).toBeTruthy();
    fireEvent.click(within(detail).getByRole("button", { name: "Open crates/api/src/main.rs in Code" }));
    expect(useEditorStore.getState().activePath).toBe("crates/api/src/main.rs");
    // Navigate along an edge.
    fireEvent.click(within(detail).getByText("api"));
    expect(screen.getByRole("complementary", { name: "Details: api" })).toBeTruthy();
  });

  it("refreshes, highlights changes and switches snapshots", async () => {
    render(<ArchitecturePanel projectId={PROJECT} />);
    await screen.findByRole("region", { name: "Architecture map" });
    fireEvent.click(screen.getByRole("button", { name: "Refresh the map" }));
    await screen.findByText("No changes: the map is up to date.");
    expect(calls.some((c) => c.method === "architecture.refresh")).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Rescan every file" }));
    await waitFor(() =>
      expect(calls.some((c) => c.method === "architecture.refresh" && (c.params as { full?: boolean }).full === true)).toBe(true),
    );

    fireEvent.click(screen.getByRole("button", { name: "Highlight changes since the previous snapshot" }));
    await screen.findByText(/added 1, changed 0, removed 1/);
    expect(screen.getByText(/Removed: legacy/)).toBeTruthy();
    fireEvent.click(screen.getByRole("tab", { name: "List" }));
    expect(screen.getAllByText("New").length).toBeGreaterThan(0);

    fireEvent.change(screen.getByLabelText("Snapshot"), { target: { value: "snap-1" } });
    await waitFor(() =>
      expect(calls.some((c) => c.method === "architecture.map" && (c.params as { snapshotId?: string }).snapshotId === "snap-1")).toBe(true),
    );
  });

  it("renders in Russian", async () => {
    await act(async () => {
      await applyLanguagePreference("ru");
    });
    render(<ArchitecturePanel projectId={PROJECT} />);
    await screen.findByRole("region", { name: "Карта архитектуры" });
    expect(screen.getByRole("button", { name: "Базы данных: 1" })).toBeTruthy();
    expect(screen.getByText("12 файлов")).toBeTruthy();
  });
});
