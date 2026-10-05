/**
 * Speech bubbles: which agent events the robot comments on, how loudly
 * (design `behaviour.bubble`) and for how long. Pure — the robot window
 * renders the returned i18n key (namespace `robots`, `bubble.*`).
 */
import type { AgentState, BubbleVerbosity, Event } from "@orkestr/protocol";

export type BubbleKey =
  | "completed"
  | "error"
  | "errorDetail"
  | "waiting"
  | "approval"
  | "thinking"
  | "coding"
  | "testing"
  | "reviewing"
  | "listening"
  | "activity"
  | "stopped"
  | "limitReached"
  | "testsPassed"
  | "testsFailed"
  | "reviewApproved"
  | "reviewChanges"
  | "reviewRejected"
  | "buildSucceeded"
  | "buildFailed";

export type BubbleTone = "info" | "success" | "warning" | "danger";

export interface BubbleSpec {
  key: BubbleKey;
  /** Interpolation values (`count`, `detail`, `activity`). */
  values: Record<string, string | number>;
  tone: BubbleTone;
  /** Auto-hide after this many ms (0 = until replaced). */
  ttlMs: number;
  /** Shown in `Minimal` (results and problems). */
  important: boolean;
  /** Shown only in `Chatty`. */
  chatty: boolean;
}

const spec = (
  key: BubbleKey,
  tone: BubbleTone,
  ttlMs: number,
  opts: { important?: boolean; chatty?: boolean; values?: Record<string, string | number> } = {},
): BubbleSpec => ({ key, tone, ttlMs, values: opts.values ?? {}, important: opts.important ?? false, chatty: opts.chatty ?? false });

/** The bubble an event produces (before verbosity filtering), or null. */
export function bubbleForEvent(ev: Event): BubbleSpec | null {
  switch (ev.kind) {
    case "AgentStateChanged": {
      const activity = ev.payload.activity ?? "";
      switch (ev.payload.to) {
        case "Completed":
          return spec("completed", "success", 12000, { important: true });
        case "Error":
          return activity
            ? spec("errorDetail", "danger", 15000, { important: true, values: { detail: activity } })
            : spec("error", "danger", 15000, { important: true });
        case "Waiting":
          return spec("waiting", "warning", 10000, { important: true });
        case "Testing":
          return spec("testing", "info", 5000);
        case "Reviewing":
          return spec("reviewing", "info", 5000);
        case "Coding":
          return activity ? spec("activity", "info", 5000, { chatty: true, values: { activity } }) : spec("coding", "info", 4000);
        case "Thinking":
          return spec("thinking", "info", 3500, { chatty: true });
        case "Listening":
          return spec("listening", "info", 3500);
        default:
          return null;
      }
    }
    case "AgentUpdated": {
      const activity = ev.payload.agent.activity;
      return activity ? spec("activity", "info", 5000, { chatty: true, values: { activity } }) : null;
    }
    case "AgentStopped":
      if (ev.payload.reason === "UserRequested" || ev.payload.reason === "EmergencyStop") {
        return spec("stopped", "warning", 8000, { important: true });
      }
      if (ev.payload.reason === "LimitReached") return spec("limitReached", "warning", 12000, { important: true });
      return null;
    case "ApprovalRequested":
      return spec("approval", "warning", 0, { important: true });
    // INTEGRATOR(P2): quality-pipeline events (tests, reviews, builds) carry the
    // agent in the envelope (`agentId`) once Phase 2 publishes them per agent.
    case "TestRunFinished":
      return ev.payload.failed > 0
        ? spec("testsFailed", "danger", 15000, { important: true, values: { count: ev.payload.failed } })
        : spec("testsPassed", "success", 12000, { important: true, values: { count: ev.payload.passed } });
    case "ReviewFinished":
      if (ev.payload.verdict === "Approved") return spec("reviewApproved", "success", 12000, { important: true });
      if (ev.payload.verdict === "ChangesRequested") {
        return spec("reviewChanges", "warning", 12000, { important: true, values: { count: ev.payload.findings } });
      }
      if (ev.payload.verdict === "Rejected") return spec("reviewRejected", "danger", 12000, { important: true });
      return null;
    case "BuildSucceeded":
      return spec("buildSucceeded", "success", 6000);
    case "BuildFailed":
      return spec("buildFailed", "danger", 12000, { important: true });
    default:
      return null;
  }
}

/**
 * The bubble a robot shows when its window opens on an agent that already
 * finished or needs the user (the events happened before the window
 * subscribed): Scenario H must still end with "done" next to the robot.
 */
export function initialBubble(state: AgentState, activity: string | null, verbosity: BubbleVerbosity): BubbleSpec | null {
  if (state !== "Completed" && state !== "Error" && state !== "Waiting") return null;
  const b = bubbleForEvent({
    seq: 0,
    id: "",
    ts: "",
    projectId: null,
    taskId: null,
    agentId: null,
    nodeId: null,
    correlationId: null,
    kind: "AgentStateChanged",
    payload: { agentId: "", from: "Idle", to: state, activity },
  });
  return b && passesVerbosity(b, verbosity) ? b : null;
}

/** Whether a bubble passes the design's verbosity. */
export function passesVerbosity(b: BubbleSpec, verbosity: BubbleVerbosity): boolean {
  switch (verbosity) {
    case "Off":
      return false;
    case "Minimal":
      return b.important;
    case "Normal":
      return !b.chatty;
    case "Chatty":
      return true;
  }
}

/** Bubble for an event under a verbosity level. */
export function bubbleFor(ev: Event, verbosity: BubbleVerbosity): BubbleSpec | null {
  const b = bubbleForEvent(ev);
  return b && passesVerbosity(b, verbosity) ? b : null;
}

/**
 * Whether a new bubble may replace the current one: important bubbles are
 * never replaced by chatter while they are fresh (first 3 s).
 */
export function mayReplace(current: { spec: BubbleSpec; shownAt: number } | null, next: BubbleSpec, now: number): boolean {
  if (!current) return true;
  if (current.spec.important && !next.important && now - current.shownAt < 3000) return false;
  return true;
}
