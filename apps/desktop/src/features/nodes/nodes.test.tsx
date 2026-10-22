// Nodes panel (Phase 6): registry with resources, enroll with SAS, worker-side
// "node" pairing, drain / revoke (confirmation) / policy, jobs with placement
// explanation + migrate / cancel, models import + distribute, and graceful
// unimplementedFeature handling. The core client is mocked.
import { OrkestrError } from "@orkestr/client";
import type { JobView, NodeSnapshot, StoredModel } from "@orkestr/protocol";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const calls: { method: string; params: unknown }[] = [];
let unimplementedAll = false;
let pending: { pairingId: string; deviceId: string; deviceName: string; platform: string; sas: string; claimedAt: string }[] = [];

const unimplemented = (m: string) =>
  new OrkestrError("bad_request", `\`${m}\` is not implemented yet (unimplemented-feature)`, { unimplementedFeature: m });

const GiB = 1024 ** 3;
function snap(id: string, name: string, extra: Partial<NodeSnapshot> = {}, os: "Windows" | "MacOs" | "Linux" = "Windows"): NodeSnapshot {
  return {
    node: {
      id,
      name,
      hostname: name,
      os,
      osVersion: "1",
      arch: os === "MacOs" ? "Arm64" : "X64",
      coreVersion: "0.1.0",
      status: "Online",
      resources: {
        cpuCores: 16,
        cpuUsagePercent: 25,
        memoryTotalBytes: 64 * GiB,
        memoryUsedBytes: 16 * GiB,
        diskTotalBytes: 1000 * GiB,
        diskFreeBytes: 500 * GiB,
        gpus: os === "Linux" ? [{ name: "NVIDIA GeForce RTX 4090", vramBytes: 24 * GiB }] : [],
      },
      tools: os === "MacOs" ? ["git", "xcodebuild"] : ["git", "cuda"],
      localModels: [],
      isLocal: id === "win",
      lastHeartbeatAt: "2026-10-22T10:00:00Z",
    },
    status: "Online",
    trustLevel: "standard",
    allowedProjects: ["*"],
    slots: { max: 2, used: 1 },
    leases: [{ jobId: "job-1-aaaa", epoch: 2, phase: "running", progress: 0.5 }],
    lastHeartbeatAt: "2026-10-22T10:00:00Z",
    isCoordinator: false,
    labels: {},
    modelDigests: [],
    ...extra,
  };
}

const nodes: NodeSnapshot[] = [
  snap("gpu", "gpu-box", {}, "Linux"),
  snap("win", "win", { isCoordinator: true, trustLevel: "trusted", leases: [] }),
  snap("mac", "mac-mini", { trustLevel: "restricted", leases: [] }, "MacOs"),
];

const job: JobView = {
  spec: {
    jobId: "job-1-aaaa",
    taskId: "t1",
    projectId: "p1",
    kind: { type: "command", program: "xcodebuild", args: ["-scheme", "App"], env: {} },
    requirements: { os: "MacOs", tools: ["xcodebuild"], localModels: [], labels: {} },
    estimate: { cpuCores: 1, memMb: 512, vramMb: 0, diskMb: 100, durationS: 60 },
    needs: { network: false, secrets: [] },
    priority: "Medium",
    affinity: { preferred: [], forbidden: [], stickyToCache: false },
  } as JobView["spec"],
  state: "running",
  owner: { kind: "node", nodeId: "mac" },
  epoch: 2,
  placement: { decision: "place", nodeId: "mac", score: 0.8, explanation: ["mac-mini: os macos ✓, xcodebuild ✓ → score 0.80", "win: os windows ≠ macos"] },
  progress: 0.4,
  artifacts: [],
  createdAt: "2026-10-22T10:00:00Z",
  updatedAt: "2026-10-22T10:00:05Z",
};

const model: StoredModel = {
  modelRef: "mock-coder:7b",
  digest: "sha256:abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
  manifest: { name: "mock-coder", tag: "7b", format: "mock", layers: [] },
  sizeBytes: 20 * 1024 * 1024,
  ollamaInstalled: false,
  installedAt: "2026-10-22T10:00:00Z",
};

vi.mock("../../core/client", () => ({
  getClient: () => ({
    call: (method: string, params: unknown) => {
      calls.push({ method, params });
      if (unimplementedAll && method !== "devices.list") return Promise.reject(unimplemented(method));
      switch (method) {
        case "nodes.list":
          return Promise.resolve({ nodes: [], snapshots: nodes });
        case "jobs.list":
          return Promise.resolve({ jobs: [job] });
        case "models.list":
          return Promise.resolve({ models: [model] });
        case "nodes.enroll":
          return Promise.resolve({
            enrollment: { nodeId: "new", coreId: "core-x", name: "linux-box", publicKey: "k", trustLevel: "standard", allowedProjects: ["*"], enrolledAt: "2026-10-22T10:00:00Z", sig: "s" },
            sas: "TIGER OCEAN 42",
          });
        case "nodes.drain":
        case "nodes.revoke":
        case "nodes.setPolicy":
          return Promise.resolve({ node: nodes[0] });
        case "jobs.migrate":
          return Promise.resolve({ job: { ...job, owner: { kind: "node", nodeId: "gpu" } } });
        case "jobs.cancel":
          return Promise.resolve({ job: { ...job, state: "cancelled" } });
        case "models.import":
          return Promise.resolve({ model: { ...model, modelRef: "tiny:latest" } });
        case "models.distribute":
          return Promise.resolve({ digest: model.digest, outcomes: [{ nodeId: "gpu", ok: true, bytesSent: 20 * 1024 * 1024 }] });
        case "devices.startPairing":
          return Promise.resolve({ pairingId: "pr-node", code: "NODE2345", expiresAt: "2026-10-22T10:10:00Z", coreId: "core-mac", gatewayUrl: "http://gw", coreKeyFingerprint: "fp", qr: "" });
        case "devices.list":
          return Promise.resolve({ devices: [], pending });
        case "devices.approve":
          return Promise.resolve({ deviceId: "coord", name: "win", role: "operator", remoteTerminal: false, createdAt: "2026-10-22T10:00:00Z", revoked: false });
        default:
          return Promise.reject(new Error(`unexpected ${method}`));
      }
    },
  }),
  maybeClient: () => null,
}));

import { i18n } from "../../i18n";
import { NodesPanel } from "./NodesPanel";

const lastCall = (m: string) => [...calls].reverse().find((c) => c.method === m)?.params as Record<string, unknown> | undefined;

beforeEach(async () => {
  await i18n.changeLanguage("en");
  calls.length = 0;
  unimplementedAll = false;
  pending = [];
});
afterEach(cleanup);

describe("Nodes panel", () => {
  it("lists nodes with status, trust, resources, slots and leases (coordinator first)", async () => {
    render(<NodesPanel />);
    const gpu = await screen.findByTestId("node-gpu");
    expect(within(gpu).getByTestId("node-status-gpu").textContent).toBe("Online");
    expect(within(gpu).getByTestId("node-trust-gpu").textContent).toBe("Standard");
    expect(within(gpu).getByTestId("node-gpus-gpu").textContent).toContain("RTX 4090");
    expect(within(gpu).getByTestId("node-gpus-gpu").textContent).toContain("VRAM");
    expect(within(gpu).getByTestId("node-slots-gpu").textContent).toBe("1 of 2 in use");
    expect(within(gpu).getByText(/epoch 2/)).toBeTruthy();
    const mac = screen.getByTestId("node-mac");
    expect(within(mac).getByText("xcodebuild")).toBeTruthy();
    expect(within(mac).getByText(/macOS · ARM64/)).toBeTruthy();
    const items = screen.getAllByTestId(/^node-(win|gpu|mac)$/).map((e) => e.dataset.testid);
    expect(items[0]).toBe("node-win");
    // the coordinator has no drain / revoke
    expect(screen.queryByTestId("node-revoke-win")).toBeNull();
  });

  it("drains, sets policy and revokes only after confirmation", async () => {
    render(<NodesPanel />);
    fireEvent.click(await screen.findByTestId("node-drain-gpu"));
    await waitFor(() => expect(lastCall("nodes.drain")).toEqual({ nodeId: "gpu", drain: true }));

    fireEvent.click(screen.getByTestId("node-policy-mac"));
    const form = screen.getByTestId("node-policy-form-mac");
    fireEvent.click(within(form).getByRole("radio", { name: "Trusted" }));
    fireEvent.change(within(form).getByLabelText(/Allowed projects/), { target: { value: "p1, p2" } });
    fireEvent.click(screen.getByTestId("node-policy-save-mac"));
    await waitFor(() =>
      expect(lastCall("nodes.setPolicy")).toEqual({ nodeId: "mac", trustLevel: "trusted", allowedProjects: ["p1", "p2"], maxConcurrentJobs: 2 }),
    );

    fireEvent.click(screen.getByTestId("node-revoke-gpu"));
    expect(calls.some((c) => c.method === "nodes.revoke")).toBe(false);
    fireEvent.click(screen.getByTestId("node-revoke-confirm-gpu"));
    await waitFor(() => expect(lastCall("nodes.revoke")).toEqual({ nodeId: "gpu" }));
  });

  it("enrolls a node and shows the SAS; a mismatch revokes it", async () => {
    render(<NodesPanel />);
    fireEvent.change(await screen.findByTestId("enroll-core-id"), { target: { value: "core-x" } });
    fireEvent.change(screen.getByTestId("enroll-pairing-id"), { target: { value: "pr-1" } });
    fireEvent.change(screen.getByTestId("enroll-code"), { target: { value: "ABCD2345" } });
    fireEvent.click(screen.getByTestId("enroll-submit"));
    expect((await screen.findByTestId("enroll-sas")).textContent).toContain("TIGER OCEAN 42");
    expect(lastCall("nodes.enroll")).toEqual({ coreId: "core-x", pairingId: "pr-1", code: "ABCD2345" });
    fireEvent.click(screen.getByTestId("enroll-sas-mismatch"));
    await waitFor(() => expect(lastCall("nodes.revoke")).toEqual({ nodeId: "new" }));
  });

  it("lets this computer become a node: node pairing with trust level, SAS, approve", async () => {
    render(<NodesPanel />);
    fireEvent.click(await screen.findByRole("radio", { name: "Restricted" }));
    fireEvent.click(screen.getByTestId("worker-allow"));
    expect((await screen.findByTestId("worker-code")).textContent).toBe("NODE2345");
    expect(screen.getByTestId("worker-core-id").textContent).toBe("core-mac");
    expect(lastCall("devices.startPairing")).toEqual({ kind: "node", trustLevel: "restricted" });
    pending = [{ pairingId: "pr-node", deviceId: "coord", deviceName: "win", platform: "windows", sas: "LION 7", claimedAt: "2026-10-22T10:01:00Z" }];
    expect((await screen.findByTestId("worker-sas", undefined, { timeout: 4000 })).textContent).toBe("LION 7");
    const approve = screen.getByTestId("worker-approve") as HTMLButtonElement;
    expect(approve.disabled).toBe(true);
    fireEvent.click(screen.getByRole("switch", { name: "The codes match" }));
    fireEvent.click(approve);
    await waitFor(() =>
      expect(lastCall("devices.approve")).toMatchObject({ pairingId: "pr-node", sasConfirmed: true, kind: "node", trustLevel: "restricted" }),
    );
  });

  it("shows jobs with node, explanation, and migrates / cancels", async () => {
    render(<NodesPanel />);
    fireEvent.click(await screen.findByRole("tab", { name: "Jobs" }));
    const row = await screen.findByTestId("job-job-1-aaaa");
    expect(within(row).getByTestId("job-state-job-1-aaaa").textContent).toBe("Running");
    expect(within(row).getByTestId("job-node-job-1-aaaa").textContent).toBe("On mac-mini");
    expect(within(row).getByTestId("job-explanation-job-1-aaaa").textContent).toContain("xcodebuild ✓");
    fireEvent.click(within(row).getByTestId("job-migrate-job-1-aaaa"));
    fireEvent.change(within(row).getByTestId("job-target-job-1-aaaa"), { target: { value: "gpu" } });
    fireEvent.click(within(row).getByTestId("job-migrate-confirm-job-1-aaaa"));
    await waitFor(() => expect(lastCall("jobs.migrate")).toEqual({ jobId: "job-1-aaaa", targetNodeId: "gpu" }));
    expect(await within(row).findByText("Migrating to gpu-box.")).toBeTruthy();
    fireEvent.click(within(row).getByTestId("job-cancel-job-1-aaaa"));
    await waitFor(() => expect(lastCall("jobs.cancel")).toEqual({ jobId: "job-1-aaaa", immediate: false }));
  });

  it("imports a mock model and distributes it to chosen nodes", async () => {
    render(<NodesPanel />);
    fireEvent.click(await screen.findByRole("tab", { name: "Models" }));
    fireEvent.change(await screen.findByTestId("model-import-name"), { target: { value: "tiny" } });
    fireEvent.click(screen.getByTestId("model-import-submit"));
    await waitFor(() => expect(lastCall("models.import")).toEqual({ name: "tiny", tag: "latest", mockSizeBytes: 20 * 1024 * 1024 }));
    fireEvent.click(screen.getByTestId("model-distribute-mock-coder:7b"));
    const targets = screen.getByTestId("model-targets-mock-coder:7b");
    expect(within(targets).queryByText("win")).toBeNull();
    fireEvent.click(within(targets).getByLabelText("gpu-box"));
    fireEvent.click(screen.getByTestId("model-distribute-confirm-mock-coder:7b"));
    await waitFor(() => expect(lastCall("models.distribute")).toEqual({ modelRef: "mock-coder:7b", nodeIds: ["gpu"] }));
    expect(await screen.findByText("Delivered to nodes: 1 of 1.")).toBeTruthy();
  });

  it("shows notices instead of errors while the core answers unimplementedFeature", async () => {
    unimplementedAll = true;
    render(<NodesPanel />);
    expect(await screen.findByText("Nodes are not available on this core yet.")).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
    fireEvent.click(screen.getByTestId("worker-allow"));
    expect(await screen.findByText("This action is not available on this core yet.")).toBeTruthy();
    fireEvent.click(screen.getByRole("tab", { name: "Jobs" }));
    expect(await screen.findByText("Jobs are not available on this core yet.")).toBeTruthy();
    fireEvent.click(screen.getByRole("tab", { name: "Models" }));
    expect(await screen.findByText("The model store is not available on this core yet.")).toBeTruthy();
  });

  it("renders in Russian", async () => {
    await i18n.changeLanguage("ru");
    render(<NodesPanel />);
    expect(await screen.findByRole("tab", { name: "Задания" })).toBeTruthy();
    expect(within(await screen.findByTestId("node-gpu")).getByText("В сети")).toBeTruthy();
    expect(screen.getByText("Подключить узел")).toBeTruthy();
  });
});
