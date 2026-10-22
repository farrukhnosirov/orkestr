/**
 * @orkestr/widget-sdk — runs inside a plugin widget iframe (ADR-0034).
 *
 * The widget lives in `<iframe sandbox="allow-scripts">` (opaque origin, CSP
 * without network). Its only channel is `postMessage` to the host frame;
 * every query is checked by the host bridge and again by core (Read-class,
 * non-local-only methods from the widget's `dataBinding` allowlist).
 */
import type { MethodName, ParamsOf, ResultOf } from "@orkestr/protocol";

import type {
  CreateWidgetOptions,
  HostToWidget,
  ThemeTokens,
  WidgetEnvironment,
  WidgetError,
  WidgetGridSize,
  WidgetHandle,
  WidgetToHost,
} from "./protocol";

export type * from "./protocol";

/** Default query timeout. */
export const DEFAULT_QUERY_TIMEOUT_MS = 30_000;

/** Error thrown by rejected queries. */
export class WidgetQueryError extends Error {
  readonly code: string;
  constructor(error: WidgetError) {
    super(error.message);
    this.name = "WidgetQueryError";
    this.code = error.code;
  }
}

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

/** Narrows an untrusted message to a host message (shape check only). */
export function parseHostMessage(data: unknown): HostToWidget | null {
  if (!isRecord(data) || typeof data["type"] !== "string") return null;
  switch (data["type"]) {
    case "init":
      return typeof data["widgetId"] === "string" && typeof data["instanceId"] === "string" && isRecord(data["theme"])
        ? (data as HostToWidget)
        : null;
    case "data":
      return typeof data["bindingId"] === "string" ? (data as HostToWidget) : null;
    case "event":
      return typeof data["kind"] === "string" ? (data as HostToWidget) : null;
    case "theme":
      return isRecord(data["theme"]) ? (data as HostToWidget) : null;
    case "error":
      return typeof data["reqId"] === "string" && isRecord(data["error"]) ? (data as HostToWidget) : null;
    default:
      return null;
  }
}

/** Applies theme tokens as `--ok-<name>` CSS variables on the document root. */
export function applyThemeTokens(theme: ThemeTokens, root: HTMLElement | undefined = globalThis.document?.documentElement): void {
  if (!root) return;
  for (const [name, value] of Object.entries(theme)) {
    if (/^[a-z0-9-]+$/.test(name) && typeof value === "string") root.style.setProperty(`--ok-${name}`, value);
  }
}

interface Pending {
  resolve: (v: unknown) => void;
  reject: (e: Error) => void;
  timer: ReturnType<typeof setTimeout>;
}

function defaultEnv(): WidgetEnvironment {
  return { self: window, host: window.parent };
}

/**
 * Connects the widget to its host frame.
 *
 * ```ts
 * const widget = createWidget({
 *   onInit: ({ config }) => render(config),
 *   onEvent: (kind) => kind === "AlertRaised" && refresh(),
 * });
 * widget.subscribe(["AlertRaised"]);
 * const overview = await widget.query("monitor.overview", {});
 * ```
 */
export function createWidget(opts: CreateWidgetOptions): WidgetHandle {
  const env = opts.env ?? defaultEnv();
  const timeoutMs = opts.timeoutMs ?? DEFAULT_QUERY_TIMEOUT_MS;
  const pending = new Map<string, Pending>();
  let seq = 0;
  let disposed = false;

  // The parent's origin is the app origin; the widget's own origin is opaque,
  // so `*` is the only usable target. Nothing secret is ever sent.
  const post = (msg: WidgetToHost) => {
    if (!disposed) env.host.postMessage(msg, "*");
  };

  const settle = (id: string, ok: boolean, value: unknown) => {
    const p = pending.get(id);
    if (!p) return false;
    pending.delete(id);
    clearTimeout(p.timer);
    if (ok) p.resolve(value);
    else p.reject(new WidgetQueryError(value as WidgetError));
    return true;
  };

  const onMessage = (e: MessageEvent) => {
    // Only the host frame may talk to us.
    if (e.source !== (env.host as unknown)) return;
    const msg = parseHostMessage(e.data);
    if (!msg) return;
    switch (msg.type) {
      case "init":
        if (opts.applyTheme !== false) applyThemeTokens(msg.theme);
        opts.onInit(msg);
        break;
      case "theme":
        if (opts.applyTheme !== false) applyThemeTokens(msg.theme);
        opts.onTheme?.(msg.theme);
        break;
      case "data":
        if (!settle(msg.bindingId, true, msg.result)) opts.onData?.(msg.bindingId, msg.result);
        break;
      case "error":
        settle(msg.reqId, false, msg.error);
        break;
      case "event":
        opts.onEvent?.(msg.kind, msg.payload);
        break;
    }
  };
  env.self.addEventListener("message", onMessage as EventListener);

  const handle: WidgetHandle = {
    query<M extends MethodName>(method: M, params: ParamsOf<M>): Promise<ResultOf<M>> {
      if (disposed) return Promise.reject(new WidgetQueryError({ code: "disposed", message: "widget disposed" }));
      seq += 1;
      const reqId = `q${seq}`;
      return new Promise<ResultOf<M>>((resolve, reject) => {
        const timer = setTimeout(() => {
          settle(reqId, false, { code: "timeout", message: `query ${method} timed out` });
        }, timeoutMs);
        pending.set(reqId, { resolve: resolve as (v: unknown) => void, reject, timer });
        post({ type: "query", reqId, method, params });
      });
    },
    subscribe(kinds: string[]) {
      post({ type: "subscribe", kinds: [...kinds] });
    },
    resize(size: WidgetGridSize) {
      post({ type: "resize", size: { w: size.w, h: size.h } });
    },
    ready() {
      post({ type: "ready" });
    },
    dispose() {
      if (disposed) return;
      for (const id of [...pending.keys()]) settle(id, false, { code: "disposed", message: "widget disposed" });
      disposed = true;
      env.self.removeEventListener("message", onMessage as EventListener);
    },
  };
  if (opts.autoReady !== false) handle.ready();
  return handle;
}
