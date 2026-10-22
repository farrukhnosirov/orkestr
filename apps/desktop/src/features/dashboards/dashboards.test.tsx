import type { Dashboard, Event, WidgetDescriptor, WidgetInstance } from "@orkestr/protocol";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const calls: { method: string; params: Record<string, unknown> }[] = [];
let stored: Dashboard;

const PLUGIN: WidgetDescriptor = {
  id: "acme.stats/alerts",
  title: "Acme alerts",
  source: { type: "Plugin", pluginId: "p1", packageId: "acme.stats" },
  sizes: [{ w: 4, h: 3 }],
  dataBinding: { methods: ["monitor.alerts", "agents.spawn", "secrets.list"], events: ["AlertRaised"] },
  configSchema: null,
} as unknown as WidgetDescriptor;

const NOTE: WidgetDescriptor = {
  id: "builtin:note",
  title: "Note",
  source: { type: "BuiltIn" },
  sizes: [{ w: 4, h: 3 }],
  dataBinding: { methods: [], events: [] },
  configSchema: {
    type: "object",
    properties: { text: { type: "string", title: "Text", maxLength: 10 }, tone: { type: "string", enum: ["neutral", "info"] } },
    required: ["text"],
    additionalProperties: false,
  },
};

const fake = {
  call: vi.fn((method: string, params: Record<string, unknown>) => {
    calls.push({ method, params });
    switch (method) {
      case "dashboards.list":
        return Promise.resolve({ dashboards: [{ id: stored.id, name: stored.name, widgetCount: stored.widgets.length, updatedAt: stored.updatedAt }] });
      case "dashboards.get":
        return Promise.resolve({ dashboard: stored });
      case "dashboards.save": {
        stored = { ...stored, name: params["name"] as string, widgets: params["widgets"] as WidgetInstance[], version: stored.version + 1 };
        return Promise.resolve({ dashboard: stored });
      }
      case "widgets.list":
        return Promise.resolve({ widgets: [NOTE, PLUGIN] });
      case "widgets.bundle":
        return Promise.resolve({ widgetId: params["widgetId"], html: "<!doctype html><html><head><title>w</title></head><body>hi</body></html>", csp: "x", sha256: "0" });
      default:
        return Promise.resolve({});
    }
  }),
  subscribe: vi.fn(() => () => {}),
};

vi.mock("../../core/client", () => ({ getClient: () => fake, maybeClient: () => fake }));

import { useAppStore } from "../../store/app";
import { createWidgetBridge, parseWidgetMessage } from "./bridge";
import { ConfigForm } from "./ConfigForm";
import { configFields, defaultConfig, validateConfig } from "./configSchema";
import { DashboardBuilder } from "./DashboardBuilder";
import { buildWidgetDocument, setWidgetDocumentLoader, WIDGET_CSP } from "./frame";
import { addItem, compact, moveItem, overlaps, resizeItem } from "./grid";
import { bindingViolation, isBindableMethod } from "./policy";
import { WidgetFrame } from "./WidgetFrame";

function inst(id: string, x: number, y: number, w: number, h: number, widgetId = "builtin:note"): WidgetInstance {
  return { instanceId: id, widgetId, x, y, w, h, config: { text: id } };
}

beforeEach(() => {
  calls.length = 0;
  fake.call.mockClear();
  stored = {
    id: "d1",
    name: "Main",
    widgets: [inst("a", 0, 0, 4, 3), inst("b", 4, 0, 4, 3)],
    version: 1,
    createdAt: "2026-10-22T10:00:00Z",
    updatedAt: "2026-10-22T10:00:00Z",
  } as Dashboard;
  useAppStore.setState((s) => ({ connection: { ...s.connection, state: "open" } }));
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

// ── policy + bridge ─────────────────────────────────────────────────────

describe("binding policy", () => {
  it("allows Read-class, non-local methods only", () => {
    expect(isBindableMethod("monitor.alerts")).toBe(true);
    expect(isBindableMethod("agents.list")).toBe(true);
    expect(isBindableMethod("dashboard.snapshot")).toBe(true);
    expect(isBindableMethod("agents.spawn")).toBe(false); // write / expensive
    expect(isBindableMethod("deploy.start")).toBe(false);
    expect(isBindableMethod("secrets.list")).toBe(false); // local-only area
    expect(isBindableMethod("vscode.status")).toBe(false);
    expect(isBindableMethod("emergency.stopAll")).toBe(false);
    expect(isBindableMethod("bad")).toBe(false);
    expect(bindingViolation({ methods: ["agents.list"] }, "monitor.alerts")).toMatch(/not in the widget/);
  });
});

function bridgeSetup() {
  const target = { postMessage: vi.fn() } as unknown as Window;
  let handler: ((e: Event) => void) | null = null;
  const client = {
    call: vi.fn(() => Promise.resolve({ alerts: [] })),
    subscribe: vi.fn((_t: string[], h: (e: Event) => void) => {
      handler = h;
      return () => {
        handler = null;
      };
    }),
  };
  const onResize = vi.fn();
  const bridge = createWidgetBridge({
    widgetId: PLUGIN.id,
    instanceId: "i1",
    dataBinding: PLUGIN.dataBinding,
    config: { a: 1 },
    locale: "en",
    topics: ["global"],
    theme: () => ({ "color-bg": "#000" }),
    target: () => target,
    client,
    onResize,
  });
  const send = (data: unknown, source: unknown = target, origin = "null") =>
    bridge.handle({ data, source, origin } as unknown as MessageEvent);
  const posted = () => (target.postMessage as ReturnType<typeof vi.fn>).mock.calls.map((c) => c[0] as Record<string, unknown>);
  return { bridge, client, send, posted, onResize, emit: (ev: Partial<Event>) => handler?.(ev as Event) };
}

describe("widget bridge", () => {
  it("rejects a write method even when it is in the binding", async () => {
    const { send, posted, client } = bridgeSetup();
    send({ type: "query", reqId: "q1", method: "agents.spawn", params: {} });
    await Promise.resolve();
    expect(client.call).not.toHaveBeenCalled();
    expect(posted()).toEqual([{ type: "error", reqId: "q1", error: expect.objectContaining({ code: "forbidden" }) }]);
  });

  it("rejects a local-only method", async () => {
    const { send, posted, client } = bridgeSetup();
    send({ type: "query", reqId: "q2", method: "secrets.list", params: {} });
    await Promise.resolve();
    expect(client.call).not.toHaveBeenCalled();
    expect(posted()[0]).toMatchObject({ type: "error", reqId: "q2", error: { code: "forbidden" } });
  });

  it("rejects methods outside the binding and answers allowed reads", async () => {
    const { send, posted, client } = bridgeSetup();
    send({ type: "query", reqId: "q3", method: "agents.list", params: {} });
    send({ type: "query", reqId: "q4", method: "monitor.alerts", params: {} });
    await waitFor(() => expect(posted()).toHaveLength(2));
    expect(client.call).toHaveBeenCalledTimes(1);
    expect(client.call).toHaveBeenCalledWith("monitor.alerts", {});
    expect(posted()).toContainEqual({ type: "data", bindingId: "q4", result: { alerts: [] } });
    expect(posted()).toContainEqual(expect.objectContaining({ type: "error", reqId: "q3" }));
  });

  it("ignores foreign sources and non-opaque origins", async () => {
    const { send, posted, client } = bridgeSetup();
    send({ type: "query", reqId: "q5", method: "monitor.alerts", params: {} }, window);
    send({ type: "query", reqId: "q6", method: "monitor.alerts", params: {} }, undefined, "http://evil");
    send({ type: "ready" }, window);
    await Promise.resolve();
    expect(client.call).not.toHaveBeenCalled();
    expect(posted()).toEqual([]);
  });

  it("sends init on ready and forwards only subscribed, bound events", () => {
    const { send, posted, emit, client, onResize } = bridgeSetup();
    send({ type: "ready" });
    expect(posted()[0]).toMatchObject({ type: "init", widgetId: PLUGIN.id, instanceId: "i1", config: { a: 1 }, theme: { "color-bg": "#000" } });
    send({ type: "subscribe", kinds: ["AlertRaised", "SecretChanged"] });
    expect(client.subscribe).toHaveBeenCalledWith(["global"], expect.any(Function));
    expect(posted()[1]).toMatchObject({ type: "error", reqId: "subscribe" });
    emit({ kind: "AlertRaised", payload: { id: 1 } } as never);
    emit({ kind: "AgentSpawned", payload: {} } as never);
    expect(posted().slice(2)).toEqual([{ type: "event", kind: "AlertRaised", payload: { id: 1 } }]);
    send({ type: "resize", size: { w: 6, h: 4 } });
    send({ type: "resize", size: { w: 60, h: 4 } });
    expect(onResize).toHaveBeenCalledTimes(1);
  });

  it("parses only well-formed widget messages", () => {
    expect(parseWidgetMessage({ type: "query", reqId: 1, method: "x" })).toBeNull();
    expect(parseWidgetMessage({ type: "resize", size: { w: 1.5, h: 1 } })).toBeNull();
    expect(parseWidgetMessage({ type: "subscribe", kinds: [1] })).toBeNull();
    expect(parseWidgetMessage({ type: "ready" })).toEqual({ type: "ready" });
  });
});

// ── iframe isolation ────────────────────────────────────────────────────

describe("WidgetFrame", () => {
  it("renders a sandboxed iframe without allow-same-origin and with the injected CSP", async () => {
    (window as unknown as Record<string, unknown>)["__TAURI_INTERNALS__"] = { invoke: () => {} };
    try {
      render(<WidgetFrame descriptor={PLUGIN} instanceId="i1" config={{}} topics={["global"]} />);
      const frame = await waitFor(() => {
        const f = document.querySelector("iframe");
        if (!f) throw new Error("no iframe yet");
        return f;
      });
      const tokens = (frame.getAttribute("sandbox") ?? "").split(/\s+/);
      expect(tokens).toEqual(["allow-scripts"]);
      expect(tokens).not.toContain("allow-same-origin");
      const doc = frame.getAttribute("srcdoc") ?? "";
      expect(doc.indexOf("Content-Security-Policy")).toBeLessThan(doc.indexOf("<title>"));
      expect(doc).toContain("default-src &#39;none&#39;".replace(/&#39;/g, "'"));
      expect(doc).toContain("connect-src 'none'");
      expect(doc).not.toContain("__TAURI");
      // The frame's window is a different global: no Tauri bridge, no parent DOM.
      const win = frame.contentWindow as (Window & Record<string, unknown>) | null;
      if (win) {
        expect(win["__TAURI_INTERNALS__"]).toBeUndefined();
        expect(win.document).not.toBe(document);
      }
      expect(fake.call).toHaveBeenCalledWith("widgets.bundle", { widgetId: PLUGIN.id });
    } finally {
      delete (window as unknown as Record<string, unknown>)["__TAURI_INTERNALS__"];
    }
  });

  it("uses the app shell's document loader (own-origin URL with CSP header) when registered", async () => {
    const loader = vi.fn(() => Promise.resolve("http://orkestr-widget.localhost/doc/1"));
    setWidgetDocumentLoader(loader);
    try {
      render(<WidgetFrame descriptor={PLUGIN} instanceId="i2" config={{}} topics={["global"]} />);
      const frame = await waitFor(() => {
        const f = document.querySelector("iframe");
        if (!f) throw new Error("no iframe yet");
        return f;
      });
      expect(frame.getAttribute("src")).toBe("http://orkestr-widget.localhost/doc/1");
      expect(frame.hasAttribute("srcdoc")).toBe(false);
      expect(frame.getAttribute("sandbox")).toBe("allow-scripts");
      expect(loader).toHaveBeenCalledWith(expect.objectContaining({ widgetId: PLUGIN.id, instanceId: "i2", csp: WIDGET_CSP }));
    } finally {
      setWidgetDocumentLoader(null);
    }
  });

  it("injects the CSP into documents without a head", () => {
    expect(buildWidgetDocument("<p>x</p>")).toMatch(/^<!doctype html><html><head><meta http-equiv="Content-Security-Policy"/);
    expect(buildWidgetDocument("<html><body>x</body></html>")).toContain(`<html><head><meta http-equiv="Content-Security-Policy" content="${WIDGET_CSP}"`);
  });
});

// ── grid ────────────────────────────────────────────────────────────────

describe("grid", () => {
  const noOverlap = (items: WidgetInstance[]) => {
    for (let i = 0; i < items.length; i += 1) for (let j = i + 1; j < items.length; j += 1) expect(overlaps(items[i]!, items[j]!)).toBe(false);
  };

  it("pushes colliding widgets and compacts", () => {
    const items = [inst("a", 0, 0, 6, 2), inst("b", 6, 0, 6, 2), inst("c", 0, 2, 12, 2)];
    const moved = moveItem(items, "b", 0, 0);
    noOverlap(moved);
    expect(moved.find((i) => i.instanceId === "b")).toMatchObject({ x: 0, y: 0 });
    const grown = resizeItem(items, "a", 12, 2);
    noOverlap(grown);
    expect(grown.find((i) => i.instanceId === "a")).toMatchObject({ w: 12, y: 0 });
    expect(compact([inst("z", 0, 7, 3, 1)])[0]).toMatchObject({ y: 0 });
  });

  it("swaps when moving down onto a widget", () => {
    const items = [inst("a", 0, 0, 12, 2), inst("b", 0, 2, 12, 2)];
    const moved = moveItem(items, "a", 0, 2);
    expect(moved.find((i) => i.instanceId === "b")).toMatchObject({ y: 0 });
    expect(moved.find((i) => i.instanceId === "a")).toMatchObject({ y: 2 });
  });

  it("adds at the first free spot and clamps to 12 columns", () => {
    const next = addItem([inst("a", 0, 0, 8, 2)], { instanceId: "n", widgetId: "w", w: 4, h: 2, config: {} });
    expect(next[1]).toMatchObject({ x: 8, y: 0 });
    const wide = resizeItem(next, "n", 40, 2);
    expect(wide.find((i) => i.instanceId === "n")).toMatchObject({ w: 12, x: 0 });
  });
});

// ── config form ─────────────────────────────────────────────────────────

describe("config schema + form", () => {
  it("validates the same subset as core", () => {
    expect(validateConfig(NOTE.configSchema, { text: "ok" })).toEqual([]);
    expect(validateConfig(NOTE.configSchema, {}).map((i) => i.code)).toEqual(["required"]);
    expect(validateConfig(NOTE.configSchema, { text: "x", tone: "loud" }).map((i) => i.code)).toEqual(["enum"]);
    expect(validateConfig(NOTE.configSchema, { text: "x", extra: 1 }).map((i) => i.code)).toEqual(["unknown"]);
    expect(validateConfig({ oneOf: [] }, {}).map((i) => i.code)).toEqual(["unsupported"]);
    expect(validateConfig(null, { a: 1 }).map((i) => i.code)).toEqual(["noConfig"]);
    expect(configFields(NOTE.configSchema).map((f) => [f.key, f.kind])).toEqual([["text", "text"], ["tone", "select"]]);
    expect(defaultConfig({ type: "object", properties: { n: { type: "integer", default: 3 } } })).toEqual({ n: 3 });
  });

  it("blocks invalid input and submits valid input", () => {
    const onSubmit = vi.fn();
    render(<ConfigForm schema={NOTE.configSchema} value={{}} onSubmit={onSubmit} onCancel={() => {}} />);
    const apply = screen.getByRole("button", { name: /apply/i });
    fireEvent.click(apply);
    expect(onSubmit).not.toHaveBeenCalled();
    expect(screen.getAllByRole("alert").length).toBeGreaterThan(0);
    const text = screen.getByLabelText(/^Text/);
    fireEvent.change(text, { target: { value: "hello" } });
    fireEvent.click(apply);
    expect(onSubmit).toHaveBeenCalledWith({ text: "hello" }, undefined);
  });
});

// ── builder persistence ─────────────────────────────────────────────────

describe("DashboardBuilder", () => {
  const saves = () => calls.filter((c) => c.method === "dashboards.save").map((c) => c.params);

  async function openEditor() {
    render(<DashboardBuilder />);
    await screen.findByLabelText("Move Note", {}, { timeout: 2000 }).catch(() => null);
    fireEvent.click(await screen.findByRole("button", { name: /^edit$/i }));
    return screen.findAllByRole("button", { name: /^Move /i });
  }

  it("persists a pointer drag via dashboards.save", async () => {
    const grips = await openEditor();
    const grid = screen.getByTestId("dashboard-grid");
    vi.spyOn(grid, "getBoundingClientRect").mockReturnValue({ width: 1188, height: 600, top: 0, left: 0, right: 1188, bottom: 600, x: 0, y: 0, toJSON: () => ({}) });
    // colPx = (1188 + 12) / 12 = 100; drag "a" right by 8 columns and down 1 row.
    act(() => {
      fireEvent.pointerDown(grips[0]!, { button: 0, clientX: 10, clientY: 10, pointerId: 1 });
    });
    act(() => {
      fireEvent.pointerMove(grid, { clientX: 810, clientY: 86, pointerId: 1 });
    });
    act(() => {
      fireEvent.pointerUp(grid, { clientX: 810, clientY: 86, pointerId: 1 });
    });
    await waitFor(() => expect(saves()).toHaveLength(1));
    const saved = saves()[0]!;
    expect(saved).toMatchObject({ dashboardId: "d1", name: "Main", expectedVersion: 1 });
    const a = (saved["widgets"] as WidgetInstance[]).find((w) => w.instanceId === "a")!;
    expect(a.x).toBe(8);
    expect(stored.version).toBe(2);
  });

  it("persists a keyboard resize and a pointer resize", async () => {
    const grips = await openEditor();
    fireEvent.keyDown(grips[1]!, { key: "ArrowRight", shiftKey: true });
    await waitFor(() => expect(saves()).toHaveLength(1));
    expect((saves()[0]!["widgets"] as WidgetInstance[]).find((w) => w.instanceId === "b")).toMatchObject({ w: 5, h: 3 });

    const grid = screen.getByTestId("dashboard-grid");
    vi.spyOn(grid, "getBoundingClientRect").mockReturnValue({ width: 1188, height: 600, top: 0, left: 0, right: 1188, bottom: 600, x: 0, y: 0, toJSON: () => ({}) });
    const handle = screen.getAllByRole("button", { name: /^Resize /i })[0]!;
    act(() => {
      fireEvent.pointerDown(handle, { button: 0, clientX: 0, clientY: 0, pointerId: 2 });
    });
    act(() => {
      fireEvent.pointerMove(grid, { clientX: 0, clientY: 152, pointerId: 2 });
    });
    act(() => {
      fireEvent.pointerUp(grid, { pointerId: 2 });
    });
    await waitFor(() => expect(saves()).toHaveLength(2));
    expect((saves()[1]!["widgets"] as WidgetInstance[]).find((w) => w.instanceId === "a")).toMatchObject({ w: 4, h: 5 });
    expect(saves()[1]).toMatchObject({ expectedVersion: 2 });
  });
});
