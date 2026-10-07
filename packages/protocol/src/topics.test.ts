import { describe, expect, it } from "vitest";
import type { Event } from "./index";
import { EVENT_KINDS, METHOD_NAMES, eventMatches, eventTopics, isEventKind, isValidTopic, topic } from "./index";

const P = "0190f0b0-0000-7000-8000-000000000001";
const T = "0190f0b0-0000-7000-8000-000000000002";

function logEvent(projectId: string | null): Event {
  return {
    seq: 1,
    id: "0190f0b0-0000-7000-8000-0000000000aa",
    ts: "2026-10-02T00:00:00Z",
    projectId,
    taskId: null,
    agentId: null,
    nodeId: null,
    correlationId: null,
    kind: "Log",
    payload: { level: "info", target: "t", message: "m", fields: null },
  };
}

describe("topics", () => {
  it("routes Claude Code chat items only to their session topic", () => {
    const S = "0190f0b0-0000-7000-8000-0000000000cc";
    const ev = {
      ...logEvent(P),
      kind: "ClaudeCodeEvent",
      payload: { sessionId: S, projectId: P, event: { type: "thinking" } },
    } as unknown as Event;
    expect(eventTopics(ev)).toEqual([topic.claude(S)]);
    expect(isValidTopic(topic.claude(S))).toBe(true);
    expect(isValidTopic("claude:nope")).toBe(false);
  });
  it("routes global events to global only", () => {
    expect(eventTopics(logEvent(null))).toEqual(["global"]);
    expect(eventMatches(["global"], logEvent(null))).toBe(true);
    expect(eventMatches([topic.project(P)], logEvent(null))).toBe(false);
    expect(eventMatches(["*"], logEvent(null))).toBe(true);
  });

  it("routes project scoped events to the project", () => {
    const ev = { ...logEvent(P), taskId: T };
    expect(eventTopics(ev)).toEqual([`project:${P}`, `task:${T}`]);
    expect(eventMatches([topic.task(T)], ev)).toBe(true);
  });

  it("routes terminal output to its terminal topic only", () => {
    const ev: Event = {
      ...logEvent(P),
      kind: "TerminalOutput",
      payload: { terminalId: T, data: "x", offset: 0 },
    };
    expect(eventTopics(ev)).toEqual([`terminal:${T}`]);
    expect(eventMatches([topic.project(P)], ev)).toBe(false);
    expect(eventMatches([topic.terminal(T)], ev)).toBe(true);
  });

  it("routes node heartbeats to the node topic only", () => {
    const N = "0190f0b0-0000-7000-8000-000000000003";
    // Only the routing-relevant fields matter here.
    const ev = { ...logEvent(null), nodeId: N, kind: "NodeStatus" } as unknown as Event;
    expect(eventTopics(ev)).toEqual([`node:${N}`]);
    expect(eventMatches(["global"], ev)).toBe(false);
    expect(eventMatches([topic.node(N)], ev)).toBe(true);
  });

  it("validates topics like the server", () => {
    expect(isValidTopic("*")).toBe(true);
    expect(isValidTopic(`project:${P}`)).toBe(true);
    expect(isValidTopic("project:nope")).toBe(false);
    expect(isValidTopic("bogus:" + P)).toBe(false);
  });

  it("narrows events by kind", () => {
    const ev = logEvent(null);
    expect(isEventKind(ev, "Log")).toBe(true);
    if (isEventKind(ev, "Log")) expect(ev.payload.level).toBe("info");
  });

  it("exposes generated constants", () => {
    expect(METHOD_NAMES).toContain("projects.create");
    expect(EVENT_KINDS).toContain("ProjectCreated");
    expect(EVENT_KINDS.length).toBeGreaterThanOrEqual(39);
  });
});
