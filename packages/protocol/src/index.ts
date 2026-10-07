/**
 * @orkestr/protocol — the Desktop/Web/Mobile shared wire protocol.
 *
 * Everything under `./generated` is produced from the Rust crates
 * (`orkestr-domain`, `orkestr-protocol`) by `npm run gen:protocol`; never edit
 * it by hand. This file adds typed helpers on top:
 *
 * - `Methods` (generated) maps `"area.verb"` → `[Params, Result]`;
 *   `MethodName`, `ParamsOf`, `ResultOf` derive from it.
 * - Event helpers (`EventOf`, `PayloadOf`, `isEventKind`).
 * - Topic helpers mirroring `orkestr_protocol::topic` (see `./topics`).
 */
import type { Event, EventKind } from "./generated";
import type { Methods } from "./generated/methods";

export * from "./generated";
export * from "./topics";

/** Every implemented RPC method name. */
export type MethodName = keyof Methods;
/** Params type of a method. */
export type ParamsOf<M extends MethodName> = Methods[M][0];
/** Result type of a method. */
export type ResultOf<M extends MethodName> = Methods[M][1];

/** Event kind names (`"ProjectCreated"`, …). */
export type EventKindName = EventKind["kind"];
/** Payload type of an event kind. */
export type PayloadOf<K extends EventKindName> = Extract<EventKind, { kind: K }>["payload"];
/** An event narrowed to one kind. */
export type EventOf<K extends EventKindName> = Event & { kind: K; payload: PayloadOf<K> };

/** Type guard narrowing an event to a kind. */
export function isEventKind<K extends EventKindName>(event: Event, kind: K): event is EventOf<K> {
  return event.kind === kind;
}

/**
 * Kinds that are streamed but not persisted in the event log (mirrors
 * `EventKind::is_ephemeral` in Rust). Their `seq` numbers show up as gaps in
 * `events.replay`; that is expected and not data loss.
 */
export const EPHEMERAL_EVENT_KINDS: ReadonlySet<EventKindName> = new Set<EventKindName>([
  "TerminalOutput",
  "AgentMessage",
  "ManagerMessage",
  "NodeStatus",
  "AgentUpdated",
  "ManagerMessageAdded",
  "SchedulerUpdated",
  "VsCodeProgress",
  "ExtensionInstallProgress",
  "LspMessage",
  "LspStatus",
]);

/** Local notices from the server (e.g. "subscription lagged") carry `seq === 0`. */
export function isLocalNotice(event: Event): boolean {
  return event.seq === 0;
}
