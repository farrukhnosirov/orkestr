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

/** Options of {@link createWidget}. */
export interface CreateWidgetOptions {
  onInit: (init: WidgetInit) => void;
  onData?: (bindingId: string, result: unknown) => void;
  onEvent?: (kind: string, payload: unknown) => void;
  onTheme?: (theme: ThemeTokens) => void;
}

/** Handle returned by {@link createWidget}. */
export interface WidgetHandle {
  /** Typed read query through the host bridge. */
  query<M extends MethodName>(method: M, params: ParamsOf<M>): Promise<ResultOf<M>>;
  /** Subscribes to event kinds (must be in `dataBinding.events`). */
  subscribe(kinds: string[]): void;
}
