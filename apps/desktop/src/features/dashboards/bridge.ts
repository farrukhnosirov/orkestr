/**
 * Host side of the widget SDK protocol (ADR-0034). One bridge per plugin
 * widget iframe:
 *
 * - accepts messages only from that iframe's window with the opaque origin
 *   (`"null"`) a sandboxed `srcdoc` frame has;
 * - answers `query` only for methods in the widget's data binding that are
 *   Read-class and not local-only ({@link bindingViolation}); core enforces
 *   the same allowlist again;
 * - forwards subscribed events (only kinds in `dataBinding.events`);
 * - sends `init` on `ready`, `theme` on theme changes.
 */
import type { Event } from "@orkestr/protocol";
import type { HostToWidget, ThemeTokens, WidgetGridSize, WidgetToHost } from "@orkestr/widget-sdk";

import { bindingViolation } from "./policy";

/** What the bridge needs from the RPC client. */
export interface BridgeClient {
  call(method: string, params: unknown): Promise<unknown>;
  subscribe(topics: string[], handler: (event: Event) => void): () => void;
}

export interface BridgeOptions {
  widgetId: string;
  instanceId: string;
  dataBinding: { methods: readonly string[]; events: readonly string[] };
  config: unknown;
  locale: string;
  /** Event topics the dashboard may see (`global`, `project:<id>`). */
  topics: readonly string[];
  theme: () => ThemeTokens;
  /** The iframe's window (null before load). */
  target: () => Window | null;
  client: BridgeClient;
  onResize?: (size: WidgetGridSize) => void;
  /** Concurrent queries per widget (default 8). */
  maxInFlight?: number;
}

export interface WidgetBridge {
  /** `message` listener for the host window. */
  handle(e: MessageEvent): void;
  sendInit(): void;
  sendTheme(): void;
  dispose(): void;
}

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

/** Narrows an untrusted message to a widget message. */
export function parseWidgetMessage(data: unknown): WidgetToHost | null {
  if (!isRecord(data)) return null;
  switch (data["type"]) {
    case "query":
      return typeof data["reqId"] === "string" && data["reqId"].length <= 64 && typeof data["method"] === "string"
        ? { type: "query", reqId: data["reqId"], method: data["method"], params: data["params"] ?? {} }
        : null;
    case "subscribe":
      return Array.isArray(data["kinds"]) && data["kinds"].every((k) => typeof k === "string")
        ? { type: "subscribe", kinds: (data["kinds"] as string[]).slice(0, 64) }
        : null;
    case "resize": {
      const size = data["size"];
      if (!isRecord(size)) return null;
      const { w, h } = size;
      return Number.isInteger(w) && Number.isInteger(h) ? { type: "resize", size: { w: w as number, h: h as number } } : null;
    }
    case "ready":
      return { type: "ready" };
    default:
      return null;
  }
}

function errorOf(e: unknown): { code: string; message: string } {
  if (isRecord(e)) {
    const code = typeof e["code"] === "string" ? e["code"] : "internal";
    const message = typeof e["message"] === "string" ? e["message"] : String(e);
    return { code, message };
  }
  return { code: "internal", message: String(e) };
}

export function createWidgetBridge(opts: BridgeOptions): WidgetBridge {
  const maxInFlight = opts.maxInFlight ?? 8;
  let inFlight = 0;
  let unsubscribe: (() => void) | null = null;
  let disposed = false;

  // A sandboxed srcdoc frame has an opaque origin: "*" is the only target that
  // reaches it. We post to the exact window object, so nothing else receives it.
  const post = (msg: HostToWidget) => {
    const target = opts.target();
    if (target && !disposed) target.postMessage(msg, "*");
  };

  const query = (reqId: string, method: string, params: unknown) => {
    const violation = bindingViolation(opts.dataBinding, method);
    if (violation) {
      post({ type: "error", reqId, error: { code: "forbidden", message: violation } });
      return;
    }
    if (inFlight >= maxInFlight) {
      post({ type: "error", reqId, error: { code: "rateLimited", message: "too many concurrent widget queries" } });
      return;
    }
    inFlight += 1;
    opts.client
      .call(method, params)
      .then((result) => post({ type: "data", bindingId: reqId, result }))
      .catch((e: unknown) => post({ type: "error", reqId, error: errorOf(e) }))
      .finally(() => {
        inFlight -= 1;
      });
  };

  const subscribe = (kinds: readonly string[]) => {
    const allowed = new Set(kinds.filter((k) => opts.dataBinding.events.includes(k)));
    const refused = kinds.filter((k) => !allowed.has(k));
    if (refused.length > 0) {
      post({ type: "error", reqId: "subscribe", error: { code: "forbidden", message: `event kinds not in the data binding: ${refused.join(", ")}` } });
    }
    unsubscribe?.();
    unsubscribe = null;
    if (allowed.size === 0 || opts.topics.length === 0) return;
    unsubscribe = opts.client.subscribe([...opts.topics], (ev) => {
      if (allowed.has(ev.kind)) post({ type: "event", kind: ev.kind, payload: ev.payload });
    });
  };

  const bridge: WidgetBridge = {
    handle(e) {
      if (disposed) return;
      const target = opts.target();
      // Source check: only our iframe; origin check: a sandboxed frame without
      // allow-same-origin always reports the opaque origin "null".
      if (!target || e.source !== target || e.origin !== "null") return;
      const msg = parseWidgetMessage(e.data);
      if (!msg) return;
      switch (msg.type) {
        case "ready":
          bridge.sendInit();
          break;
        case "query":
          query(msg.reqId, msg.method, msg.params);
          break;
        case "subscribe":
          subscribe(msg.kinds);
          break;
        case "resize":
          if (msg.size.w >= 1 && msg.size.w <= 12 && msg.size.h >= 1 && msg.size.h <= 24) opts.onResize?.(msg.size);
          break;
      }
    },
    sendInit() {
      post({
        type: "init",
        widgetId: opts.widgetId,
        instanceId: opts.instanceId,
        config: opts.config,
        theme: opts.theme(),
        locale: opts.locale,
      });
    },
    sendTheme() {
      post({ type: "theme", theme: opts.theme() });
    },
    dispose() {
      disposed = true;
      unsubscribe?.();
      unsubscribe = null;
    },
  };
  return bridge;
}
