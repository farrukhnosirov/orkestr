/**
 * Subscription topics — mirrors `orkestr_protocol::topic` (Rust).
 *
 * - every event is published on each scope topic it carries
 *   (`project:`, `task:`, `agent:`, `node:`, `terminal:`);
 * - events without a project scope, and project lifecycle events, are also
 *   published on `global`;
 * - `*` matches everything.
 */
import type { Event } from "./generated";

export const TOPIC_ALL = "*";
export const TOPIC_GLOBAL = "global";

export type ScopedTopic =
  | `project:${string}`
  | `task:${string}`
  | `agent:${string}`
  | `terminal:${string}`
  | `node:${string}`;
export type Topic = typeof TOPIC_ALL | typeof TOPIC_GLOBAL | ScopedTopic;

export const topic = {
  all: TOPIC_ALL,
  global: TOPIC_GLOBAL,
  project: (id: string): ScopedTopic => `project:${id}`,
  task: (id: string): ScopedTopic => `task:${id}`,
  agent: (id: string): ScopedTopic => `agent:${id}`,
  terminal: (id: string): ScopedTopic => `terminal:${id}`,
  node: (id: string): ScopedTopic => `node:${id}`,
} as const;

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const SCOPES = new Set(["project", "task", "agent", "terminal", "node"]);

/** Whether the server will accept this topic. */
export function isValidTopic(t: string): t is Topic {
  if (t === TOPIC_ALL || t === TOPIC_GLOBAL) return true;
  const idx = t.indexOf(":");
  if (idx < 0) return false;
  return SCOPES.has(t.slice(0, idx)) && UUID_RE.test(t.slice(idx + 1));
}

const LIFECYCLE = new Set(["ProjectCreated", "ProjectUpdated", "ProjectDeleted"]);

/** Topics an event is published on (same rules as the server). */
export function eventTopics(event: Event): string[] {
  const out: string[] = [];
  // High-volume terminal output goes only to its terminal topic.
  if (event.kind === "TerminalOutput") return [topic.terminal(event.payload.terminalId)];
  if (event.projectId == null || LIFECYCLE.has(event.kind)) out.push(TOPIC_GLOBAL);
  if (event.projectId != null) out.push(topic.project(event.projectId));
  if (event.taskId != null) out.push(topic.task(event.taskId));
  if (event.agentId != null) out.push(topic.agent(event.agentId));
  if (event.nodeId != null) out.push(topic.node(event.nodeId));
  if (event.kind === "TerminalExited") {
    out.push(topic.terminal(event.payload.terminalId));
  }
  return out;
}

/** Whether an event matches any of the topics. */
export function eventMatches(topics: Iterable<string>, event: Event): boolean {
  const list = [...topics];
  if (list.includes(TOPIC_ALL)) return true;
  const evTopics = eventTopics(event);
  return list.some((t) => evTopics.includes(t));
}
