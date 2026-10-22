/**
 * @orkestr/client — the one way clients (desktop, web, mobile) talk to the
 * Orkestr core: typed RPC calls, topic subscriptions, auto-reconnect.
 */
export { OrkestrClient } from "./client";
export type {
  CallOptions,
  ClientOptions,
  ConnectionState,
  EventHandler,
  StateListener,
  Unsubscribe,
  WebSocketFactory,
  WebSocketLike,
} from "./client";
export { OrkestrError } from "./errors";
export type { ClientErrorCode } from "./errors";
export { backoffDelay } from "./backoff";
export type { BackoffOptions } from "./backoff";

// Remote access (Phase 4, package C).
export * from "./remote";
