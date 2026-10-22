import { afterEach, describe, expect, it, vi } from "vitest";

import { applyThemeTokens, createWidget, parseHostMessage, WidgetQueryError } from "./index";
import type { WidgetEnvironment, WidgetToHost } from "./protocol";

class FakeWindow {
  listeners = new Set<(e: MessageEvent) => void>();
  addEventListener(_t: string, l: (e: MessageEvent) => void) {
    this.listeners.add(l);
  }
  removeEventListener(_t: string, l: (e: MessageEvent) => void) {
    this.listeners.delete(l);
  }
  dispatch(data: unknown, source: unknown) {
    for (const l of [...this.listeners]) l({ data, source } as MessageEvent);
  }
}

function setup(extra: Partial<Parameters<typeof createWidget>[0]> = {}) {
  const self = new FakeWindow();
  const sent: WidgetToHost[] = [];
  const host = { postMessage: vi.fn((m: WidgetToHost) => sent.push(m)) };
  const env = { self, host } as unknown as WidgetEnvironment;
  const onInit = vi.fn();
  const widget = createWidget({ onInit, env, applyTheme: false, ...extra });
  const fromHost = (data: unknown) => self.dispatch(data, host);
  return { self, host, sent, widget, onInit, fromHost };
}

afterEach(() => vi.useRealTimers());

describe("createWidget", () => {
  it("announces ready and receives init only from the host", () => {
    const { sent, onInit, self, fromHost } = setup();
    expect(sent).toEqual([{ type: "ready" }]);
    const init = { type: "init", widgetId: "p/w", instanceId: "i1", config: { a: 1 }, theme: {}, locale: "en" };
    self.dispatch(init, {}); // foreign source
    expect(onInit).not.toHaveBeenCalled();
    fromHost(init);
    expect(onInit).toHaveBeenCalledWith(init);
  });

  it("resolves and rejects queries by reqId", async () => {
    const { sent, widget, fromHost } = setup();
    const p1 = widget.query("monitor.overview", {} as never);
    const p2 = widget.query("agents.list", {} as never);
    const [q1, q2] = sent.slice(1) as Extract<WidgetToHost, { type: "query" }>[];
    expect(q1).toMatchObject({ type: "query", method: "monitor.overview" });
    fromHost({ type: "data", bindingId: q1!.reqId, result: { ok: true } });
    fromHost({ type: "error", reqId: q2!.reqId, error: { code: "forbidden", message: "nope" } });
    await expect(p1).resolves.toEqual({ ok: true });
    const err = await p2.catch((e: unknown) => e);
    expect(err).toBeInstanceOf(WidgetQueryError);
    expect((err as WidgetQueryError).code).toBe("forbidden");
  });

  it("routes pushed data, events and theme", () => {
    const onData = vi.fn();
    const onEvent = vi.fn();
    const onTheme = vi.fn();
    const { fromHost } = setup({ onData, onEvent, onTheme });
    fromHost({ type: "data", bindingId: "b1", result: 3 });
    fromHost({ type: "event", kind: "AlertRaised", payload: { x: 1 } });
    fromHost({ type: "theme", theme: { "color-bg": "#000" } });
    fromHost({ type: "bogus" });
    expect(onData).toHaveBeenCalledWith("b1", 3);
    expect(onEvent).toHaveBeenCalledWith("AlertRaised", { x: 1 });
    expect(onTheme).toHaveBeenCalledWith({ "color-bg": "#000" });
  });

  it("sends subscribe / resize and times out queries", async () => {
    vi.useFakeTimers();
    const { sent, widget } = setup({ timeoutMs: 50 });
    widget.subscribe(["AlertRaised"]);
    widget.resize({ w: 6, h: 4 });
    expect(sent.slice(1)).toEqual([
      { type: "subscribe", kinds: ["AlertRaised"] },
      { type: "resize", size: { w: 6, h: 4 } },
    ]);
    const q = widget.query("monitor.overview", {} as never).catch((e: WidgetQueryError) => e.code);
    vi.advanceTimersByTime(60);
    await expect(q).resolves.toBe("timeout");
  });

  it("dispose rejects pending queries and stops listening", async () => {
    const { widget, self } = setup();
    const q = widget.query("monitor.overview", {} as never).catch((e: WidgetQueryError) => e.code);
    widget.dispose();
    await expect(q).resolves.toBe("disposed");
    expect(self.listeners.size).toBe(0);
  });
});

describe("helpers", () => {
  it("parseHostMessage rejects malformed input", () => {
    expect(parseHostMessage(null)).toBeNull();
    expect(parseHostMessage({ type: "init" })).toBeNull();
    expect(parseHostMessage({ type: "error", reqId: "x", error: "str" })).toBeNull();
    expect(parseHostMessage({ type: "event", kind: "A", payload: 1 })).not.toBeNull();
  });

  it("applyThemeTokens sets only safe variable names", () => {
    const set = vi.fn();
    applyThemeTokens({ "color-bg": "#111", "bad name;": "x" }, { style: { setProperty: set } } as unknown as HTMLElement);
    expect(set).toHaveBeenCalledTimes(1);
    expect(set).toHaveBeenCalledWith("--ok-color-bg", "#111");
  });
});
