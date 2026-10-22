/**
 * Widget <-> host message protocol (ADR-0034). Plugin widgets run in
 * `<iframe sandbox="allow-scripts">` (opaque origin, no network via CSP);
 * the only channel is `postMessage`. The host validates every message:
 * `query` methods must be in the widget's `dataBinding.methods` allowlist and
 * Read-class / non-local-only (enforced again in core).
 *
 * Owner: WP3. These types are binding for the host bridge
 * (`apps/desktop/src/features/dashboards`).
 */
import type { MethodName, ParamsOf, ResultOf } from "@orkestr/protocol";

/** Theme tokens (`--ok-*` CSS variables without the prefix → value). */
export type ThemeTokens = Record<string, string>;

/** Widget grid size. */
export interface WidgetGridSize {
  w: number;
  h: number;
}

/** Messages the host sends to a widget. */
export type HostToWidget =
  | {
      type: "init";
      widgetId: string;
      instanceId: string;
      config: unknown;
      theme: ThemeTokens;
      locale: string;
    }
  | { type: "data"; bindingId: string; result: unknown }
  | { type: "event"; kind: string; payload: unknown }
  | { type: "theme"; theme: ThemeTokens }
  | { type: "error"; reqId: string; error: { code: string; message: string } };

/** Messages a widget sends to the host. */
export type WidgetToHost =
  | { type: "query"; reqId: string; method: string; params: unknown }
  | { type: "subscribe"; kinds: string[] }
  | { type: "resize"; size: WidgetGridSize }
  | { type: "ready" };

/** Init payload. */
export type WidgetInit = Extract<HostToWidget, { type: "init" }>;

/** Error payload of a rejected query. */
export interface WidgetError {
  code: string;
  message: string;
}

/**
 * Query replies reuse the `data` / `error` messages: `bindingId` / `reqId`
 * equal the `reqId` of the `query`. A `data` message whose `bindingId` is not
 * a pending query is pushed data (delivered to `onData`).
 */
export type QueryReply = Extract<HostToWidget, { type: "data" | "error" }>;

/** Where the SDK talks to (defaults: `window` and `window.parent`). */
export interface WidgetEnvironment {
  /** The widget's own window (receives `message` events). */
  self: Pick<Window, "addEventListener" | "removeEventListener">;
  /** The host window. */
  host: Pick<Window, "postMessage">;
}

/** Options of {@link createWidget}. */
export interface CreateWidgetOptions {
  onInit: (init: WidgetInit) => void;
  onData?: (bindingId: string, result: unknown) => void;
  onEvent?: (kind: string, payload: unknown) => void;
  onTheme?: (theme: ThemeTokens) => void;
  /** Query timeout in ms (default 30 000). */
  timeoutMs?: number;
  /** Send `ready` right away (default true). */
  autoReady?: boolean;
  /** Apply theme tokens as `--ok-*` CSS variables on `<html>` (default true). */
  applyTheme?: boolean;
  /** Test seam. */
  env?: WidgetEnvironment;
}

/** Handle returned by {@link createWidget}. */
export interface WidgetHandle {
  /** Typed read query through the host bridge. */
  query<M extends MethodName>(method: M, params: ParamsOf<M>): Promise<ResultOf<M>>;
  /** Subscribes to event kinds (must be in `dataBinding.events`). */
  subscribe(kinds: string[]): void;
  /** Asks the host for another grid size (the host may refuse). */
  resize(size: WidgetGridSize): void;
  /** Tells the host the widget is ready for `init` (sent automatically unless `autoReady: false`). */
  ready(): void;
  /** Detaches listeners and rejects pending queries. */
  dispose(): void;
}
