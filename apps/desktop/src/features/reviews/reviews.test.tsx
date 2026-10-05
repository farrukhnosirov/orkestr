import type { ConsensusRun, Event, Review, ReviewFinding, ReviewGate } from "@orkestr/protocol";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { applyLanguagePreference } from "../../i18n";
import { useAppStore } from "../../store/app";
import { runsByDecision } from "./DecisionConsensus";
import { groupBySeverity, isBlocking, latestOf, orderedProposals, upsertReview, validateConsensus } from "./model";

const PROJECT = "0190f0b0-0000-7000-8000-ffffffffffff";
const TASK = "0190f0b0-0000-7000-8000-000000000001";
const R1 = "0190f0b0-0000-7000-8000-0000000000a1";
const R2 = "0190f0b0-0000-7000-8000-0000000000a2";
const C1 = "0190f0b0-0000-7000-8000-0000000000c1";
const C2 = "0190f0b0-0000-7000-8000-0000000000c2";
const DEC = "0190f0b0-0000-7000-8000-0000000000d1";
const P = "0190f0b0-0000-7000-8000-0000000000aa";

function finding(id: string, severity: ReviewFinding["severity"], patch: Partial<ReviewFinding> = {}): ReviewFinding {
  return {
    id,
    severity,
    category: "security",
    file: "src/auth.rs",
    line: 42,
    title: `Finding ${id}`,
    detail: "detail",
    suggestion: "fix it",
    evidence: "",
    status: "Open",
    response: null,
    resolutionNote: null,
    resolvedAt: null,
    ...patch,
  };
}

function review(id: string, patch: Partial<Review> = {}): Review {
  return {
    id,
    projectId: PROJECT,
    taskId: TASK,
    kind: "Standard",
    status: "Finished",
    verdict: "RequestChanges",
    summary: "Token check is missing.",
    findings: [finding("f1", "Critical"), finding("f2", "Low", { category: "style", file: null, line: null })],
    acCheck: [
      { criterion: "login works", met: true, evidence: "tests pass" },
      { criterion: "rate limited", met: false, evidence: "" },
    ],
    model: { providerId: P, model: "mock-smart" },
    authorModel: null,
    sameModelFallback: false,
    verifierModel: null,
    diff: { files: 2, additions: 10, deletions: 3, truncated: false, source: "main...agent/x" },
    transitionedTo: "Running",
    linkedReviewId: null,
    usage: { inputTokens: 0, outputTokens: 0, costUsd: 0 },
    error: null,
    createdAt: "2026-10-05T10:00:00Z",
    updatedAt: "2026-10-05T10:00:00Z",
    finishedAt: "2026-10-05T10:00:05Z",
    ...patch,
  };
}

const GATE: ReviewGate = {
  taskId: TASK,
  reviewerApproved: false,
  noCriticalSecurity: false,
  blockingFindings: 1,
  latestReviewId: R1,
  latestAdversarialId: null,
  passed: false,
  reasons: ["not approved"],
};

function run(patch: Partial<ConsensusRun> = {}): ConsensusRun {
  const proposal = (index: number, model: string, score: number) => ({
    index,
    model: { providerId: P, model },
    answeredBy: { providerId: P, model },
    status: "Done" as const,
    title: `Approach ${index}`,
    summary: `Summary ${index}`,
    approach: "Use **Postgres**",
    pros: ["simple"],
    cons: ["slow"],
    risks: [],
    confidence: 0.7,
    score,
    scoreNote: null,
    error: null,
    usage: { inputTokens: 0, outputTokens: 0, costUsd: 0 },
  });
  return {
    id: C1,
    projectId: PROJECT,
    taskId: null,
    question: "Which queue should we use?",
    context: "",
    purpose: null,
    status: "Finished",
    proposals: [proposal(0, "mock-fast", 6), proposal(1, "mock-smart", 8.5)],
    synthesizer: { providerId: P, model: "mock-smart" },
    decision: {
      title: "Use Postgres as the queue",
      decision: "Postgres with SKIP LOCKED.",
      reasoningSummary: "Fewer moving parts.",
      consequences: "",
      chosenIndex: 1,
      agreement: 0.8,
    },
    decisionId: DEC,
    usage: { inputTokens: 0, outputTokens: 0, costUsd: 0 },
    warnings: [],
    error: null,
    createdAt: "2026-10-05T10:00:00Z",
    updatedAt: "2026-10-05T10:00:00Z",
    finishedAt: "2026-10-05T10:00:05Z",
    ...patch,
  };
}

// ── fake client ─────────────────────────────────────────────

const handlers = new Set<(ev: Event) => void>();
const calls: Array<{ method: string; params: unknown }> = [];
let reviews: Review[] = [];
let runs: ConsensusRun[] = [];

const fakeClient = {
  call: vi.fn(async (method: string, params: unknown) => {
    calls.push({ method, params });
    switch (method) {
      case "reviews.list":
        return { reviews, gate: GATE };
      case "reviews.request":
      case "reviews.adversarial":
        return { review: review(R2, { status: "Running", verdict: null, findings: [] }), adversarial: null };
      case "reviews.resolveFinding":
        return { review: reviews[0], adversarial: null };
      case "consensus.list":
        return { runs };
      case "consensus.run":
        return { run: run({ id: C2, question: "JWT or sessions?", status: "Proposing", decision: null, decisionId: null }) };
      case "providers.list":
        return { providers: [] };
      default:
        throw new Error(`unexpected ${method}`);
    }
  }),
  subscribe: vi.fn((_topics: string[], h: (ev: Event) => void) => {
    handlers.add(h);
    return () => handlers.delete(h);
  }),
};

vi.mock("../../core/client", () => ({
  maybeClient: () => fakeClient,
  getClient: () => fakeClient,
}));

const opened: Array<{ path: string; line?: number }> = [];
vi.mock("../files/actions", () => ({
  openInEditor: (path: string, at?: { line: number }) => opened.push({ path, line: at?.line }),
}));

vi.mock("../providers", () => ({
  useProvidersLive: () => ({ providers: [] }),
  ModelPicker: ({ label }: { label: string }) => <div data-testid="model-picker">{label}</div>,
}));

const { ReviewsSection } = await import("./ReviewsSection");
const { ConsensusDialog } = await import("./ConsensusDialog");

function emit(ev: Omit<Event, "seq" | "id" | "ts" | "projectId" | "taskId" | "agentId" | "nodeId" | "correlationId">) {
  const full = { seq: 1, id: "e", ts: "2026-10-05T10:00:00Z", projectId: PROJECT, taskId: TASK, agentId: null, nodeId: null, correlationId: null, ...ev } as Event;
  act(() => handlers.forEach((h) => h(full)));
}

// ── pure helpers ────────────────────────────────────────────

describe("review model", () => {
  it("groups findings by severity, most severe first", () => {
    const groups = groupBySeverity([finding("a", "Low"), finding("b", "Critical"), finding("c", "Low")]);
    expect(groups.map((g) => [g.severity, g.findings.length])).toEqual([
      ["Critical", 1],
      ["Low", 2],
    ]);
  });

  it("knows what blocks the gate", () => {
    expect(isBlocking(finding("a", "Critical"))).toBe(true);
    expect(isBlocking(finding("a", "Critical", { status: "Confirmed" }))).toBe(true);
    expect(isBlocking(finding("a", "Critical", { status: "Rebutted" }))).toBe(false);
    expect(isBlocking(finding("a", "High"))).toBe(false);
  });

  it("keeps the newest review of each kind", () => {
    const older = review(R1, { createdAt: "2026-10-05T09:00:00Z" });
    const newer = review(R2, { createdAt: "2026-10-05T11:00:00Z" });
    const list = upsertReview([older], newer);
    expect(list.map((r) => r.id)).toEqual([R2, R1]);
    expect(latestOf(list, "Standard")?.id).toBe(R2);
    expect(latestOf(list, "Adversarial")).toBeNull();
    expect(upsertReview(list, { ...older, summary: "x" }).length).toBe(2);
  });

  it("orders proposals with the chosen one first and maps decisions to runs", () => {
    const r = run();
    expect(orderedProposals(r).map((p) => p.index)).toEqual([1, 0]);
    expect(runsByDecision([r, run({ id: "x", decisionId: null })]).get(DEC)?.id).toBe(C1);
  });

  it("validates consensus requests", () => {
    const m = (model: string) => ({ providerId: P, model });
    expect(validateConsensus("  ", [], true)).toBe("needQuestion");
    expect(validateConsensus("q", [], true)).toBeNull();
    expect(validateConsensus("q", [m("a"), m("a")], false)).toBe("needModels");
    expect(validateConsensus("q", [m("a"), m("b")], false)).toBeNull();
    expect(validateConsensus("q", ["a", "b", "c", "d", "e", "f"].map(m), false)).toBe("tooManyModels");
    expect(validateConsensus("x".repeat(8001), [], true)).toBe("tooLong");
  });
});

// ── components ──────────────────────────────────────────────

describe("ReviewsSection", () => {
  beforeEach(async () => {
    await applyLanguagePreference("en");
    calls.length = 0;
    opened.length = 0;
    handlers.clear();
    reviews = [review(R1)];
    useAppStore.getState().setConnection({ state: "open" });
  });
  afterEach(() => cleanup());

  it("shows verdict, gate, findings by severity, AC check and resolves findings", async () => {
    const toast = vi.fn();
    render(<ReviewsSection taskId={TASK} onToast={toast} />);
    await screen.findByText("Changes requested");
    expect(screen.getByText("Review gate blocked")).toBeTruthy();
    expect(screen.getByText("Critical")).toBeTruthy();
    expect(screen.getByText("Low")).toBeTruthy();
    expect(screen.getByText("Task moved to: Running")).toBeTruthy();
    expect(screen.getByText("login works")).toBeTruthy();
    expect(screen.getByText("1 blocking finding")).toBeTruthy();

    // File link opens the editor at the line.
    fireEvent.click(screen.getByText("src/auth.rs:42"));
    expect(opened).toEqual([{ path: "src/auth.rs", line: 42 }]);

    // Dismissing a critical finding needs a note.
    const critical = screen.getByText("Finding f1").closest("li") as HTMLElement;
    fireEvent.click(within(critical).getByText("Dismiss"));
    fireEvent.click(within(critical).getByText("Dismiss finding"));
    expect(within(critical).getByText("A note is required to dismiss a critical finding.")).toBeTruthy();
    expect(calls.some((c) => c.method === "reviews.resolveFinding")).toBe(false);
    fireEvent.change(within(critical).getByLabelText("Why is this not a problem?"), { target: { value: "false positive" } });
    fireEvent.click(within(critical).getByText("Dismiss finding"));
    await waitFor(() =>
      expect(calls.find((c) => c.method === "reviews.resolveFinding")?.params).toEqual({
        reviewId: R1,
        findingId: "f1",
        status: "Dismissed",
        note: "false positive",
      }),
    );
  });

  it("requests reviews and follows live updates", async () => {
    reviews = [];
    const toast = vi.fn();
    render(<ReviewsSection taskId={TASK} onToast={toast} />);
    await screen.findByText(/No reviews yet/);
    fireEvent.click(screen.getByText("Review + adversarial"));
    await waitFor(() =>
      expect(calls.find((c) => c.method === "reviews.request")?.params).toEqual({ taskId: TASK, adversarial: true }),
    );
    expect(toast).toHaveBeenCalledWith("info", "Code review started");

    emit({ kind: "ReviewUpdated", payload: { review: review(R2, { kind: "Adversarial", status: "Verifying", verdict: null }) } });
    await screen.findByText("Checking the findings with the author's model…");
    expect(screen.getByText("Adversarial")).toBeTruthy();
    // While a review runs, new requests are disabled.
    expect((screen.getByText("Request review").closest("button") as HTMLButtonElement).disabled).toBe(true);
  });

  it("renders in Russian", async () => {
    await applyLanguagePreference("ru");
    render(<ReviewsSection taskId={TASK} onToast={vi.fn()} />);
    await screen.findByText("Нужны доработки");
    expect(screen.getByText("Критическое")).toBeTruthy();
    expect(screen.getByText("1 блокирующее замечание")).toBeTruthy();
    await applyLanguagePreference("en");
  });
});

describe("ConsensusDialog", () => {
  beforeEach(async () => {
    await applyLanguagePreference("en");
    calls.length = 0;
    handlers.clear();
    runs = [run()];
    useAppStore.getState().setConnection({ state: "open" });
  });
  afterEach(() => cleanup());

  it("validates, runs by route and shows proposals side by side with the decision", async () => {
    const onClose = vi.fn();
    render(<ConsensusDialog projectId={PROJECT} onClose={onClose} />);
    // History lists the previous run.
    await screen.findByText("Which queue should we use?");

    fireEvent.click(screen.getByText("Run consensus", { selector: ".ok-btn__label" }));
    expect(await screen.findByText("Enter a question.")).toBeTruthy();

    // Manual mode needs two different models.
    fireEvent.change(screen.getByLabelText("Question or design task"), { target: { value: "JWT or sessions?" } });
    fireEvent.click(screen.getByLabelText("Choose models"));
    fireEvent.click(screen.getByText("Run consensus", { selector: ".ok-btn__label" }));
    expect(await screen.findByText("Choose at least two different models.")).toBeTruthy();

    // Routing mode: Security route, 2 models.
    fireEvent.click(screen.getByLabelText("Pick by routing"));
    const select = (label: string) => screen.getByText(label).closest("label")?.querySelector("select") as HTMLSelectElement;
    fireEvent.change(select("Route"), { target: { value: "Security" } });
    fireEvent.change(select("Number of models"), { target: { value: "2" } });
    fireEvent.click(screen.getByText("Run consensus", { selector: ".ok-btn__label" }));
    await waitFor(() =>
      expect(calls.find((c) => c.method === "consensus.run")?.params).toEqual({
        projectId: PROJECT,
        question: "JWT or sessions?",
        recordDecision: true,
        participants: 2,
        purpose: "Security",
      }),
    );

    // The finished run arrives as an event and is shown.
    // The new run is shown at once, then follows its live events.
    await screen.findByText("Models are proposing…");
    emit({ kind: "ConsensusUpdated", payload: { run: run({ id: C2, question: "JWT or sessions?" }) } });
    await screen.findByText("Use Postgres as the queue");
    expect(screen.getByText("Chosen")).toBeTruthy();
    expect(screen.getByText("Approach 0")).toBeTruthy();
    expect(screen.getByText("Approach 1")).toBeTruthy();
    expect(screen.getByText("Open Memory (Decision Log)")).toBeTruthy();
  });
});
