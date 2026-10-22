/**
 * @orkestr/widget-sdk — runs inside a plugin widget iframe (ADR-0034).
 *
 * Step-0 skeleton: the types are final; `createWidget` is implemented by WP3.
 */
import type { CreateWidgetOptions, WidgetHandle } from "./protocol";

export type * from "./protocol";

/** Error code used by skeleton functions. */
export const UNIMPLEMENTED = "unimplementedFeature";

/**
 * Connects the widget to its host frame.
 *
 * Step-0 skeleton: every query rejects with `unimplementedFeature`.
 */
export function createWidget(_opts: CreateWidgetOptions): WidgetHandle {
  return {
    query: () => Promise.reject(new Error(`${UNIMPLEMENTED}: widget-sdk.query`)),
    subscribe: () => {
      throw new Error(`${UNIMPLEMENTED}: widget-sdk.subscribe`);
    },
  };
}
