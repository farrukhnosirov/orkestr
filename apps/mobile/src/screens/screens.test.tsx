import { fireEvent, screen, waitFor } from "@testing-library/react-native";
import { nobleCrypto, setRemoteCrypto } from "@orkestr/client";

import { approval, fakeClient, stepUpRequired } from "../test/fakeClient";
import { handleNotificationTap } from "../navigation/types";
import { makeDeps, renderScreen } from "../test/renderScreen";
import { ApprovalDetailScreen, ApprovalsScreen } from "./ApprovalsScreen";
import HomeScreen from "./HomeScreen";
import PairingScreen from "./PairingScreen";

beforeAll(() => setRemoteCrypto(nobleCrypto()));

const monitoringFakes = {
  "dashboard.snapshot": {
    projects: [],
    totals: { projects: 1, activeAgents: 2, totalAgents: 3, runningTasks: 4, failedTasks: 0, pendingApprovals: 1, liveRuns: 0 },
  },
  "nodes.list": { nodes: [{ id: "n1", name: "workstation" }] },
  "nodes.resources": { sample: { cpuPercent: 12.4, memoryUsedBytes: 8 * 1024 ** 3, memoryTotalBytes: 32 * 1024 ** 3 }, intervalMs: 2000 },
  "agents.list": { agents: [{ id: "ag1", name: "backend-dev", state: "Coding", activity: null }] },
  "approvals.list": { approvals: [approval()] },
  "emergency.status": { flags: { allStopped: false }, activeAgents: 2 },
  "emergency.stopAll": { agentsStopped: 2, otherStopped: 0, approvalsCancelled: 1, flags: { allStopped: true } },
};

describe("Home / monitoring", () => {
  it("renders monitoring data", async () => {
    const client = fakeClient(monitoringFakes);
    const { deps } = makeDeps(client);
    await renderScreen(HomeScreen, { params: { coreId: "core-1" } }, deps);
    expect(await screen.findByTestId("stat-activeAgents", {}, { timeout: 5000 })).toHaveTextContent("2");
    expect(screen.getByTestId("stat-runningTasks")).toHaveTextContent("4");
    expect(screen.getByText("backend-dev")).toBeTruthy();
    expect(screen.getByText(/CPU 12%/)).toBeTruthy();
    expect(screen.getByText("workstation")).toBeTruthy();
  });

  it("emergency stop needs confirmation", async () => {
    const client = fakeClient(monitoringFakes);
    const { deps } = makeDeps(client);
    await renderScreen(HomeScreen, { params: { coreId: "core-1" } }, deps);
    await screen.findByTestId("stat-activeAgents");
    await fireEvent.press(screen.getByTestId("emergency-stop"));
    expect(client.calls.some((c) => c.method === "emergency.stopAll")).toBe(false);
    await fireEvent.press(await screen.findByTestId("confirm-cancel"));
    expect(client.calls.some((c) => c.method === "emergency.stopAll")).toBe(false);
    await fireEvent.press(screen.getByTestId("emergency-stop"));
    await fireEvent.press(await screen.findByTestId("confirm-confirm"));
    await waitFor(() => expect(client.calls.filter((c) => c.method === "emergency.stopAll")).toHaveLength(1));
    expect(await screen.findByTestId("home-notice")).toHaveTextContent("Stopped. Agents stopped: 2.");
  });
});

describe("Approvals", () => {
  it("lists pending approvals and opens the detail", async () => {
    const client = fakeClient({ "approvals.list": { approvals: [approval()] } });
    const { deps } = makeDeps(client);
    const { navigation } = await renderScreen(ApprovalsScreen, { params: { coreId: "c" } }, deps);
    await fireEvent.press(await screen.findByTestId("approval-a-1"));
    expect(navigation.navigate).toHaveBeenCalledWith("ApprovalDetail", { coreId: "c", approvalId: "a-1" });
  });

  async function setup(allow: boolean) {
    let resolves = 0;
    const client = fakeClient({
      "approvals.get": { approval: approval() },
      "approvals.resolve": (p: Record<string, unknown>) => {
        resolves += 1;
        if (!p.stepUp) throw stepUpRequired("chal");
        return { approval: approval({ status: "Approved" }) };
      },
    });
    const { core, deps } = makeDeps(client);
    await core.store.put({ id: "c", coreId: "c", deviceId: "dev-1", gatewayUrl: "g", corePubKey: "k", deviceName: "n", pairedAt: 1 });
    await core.stepUpKeys.create("c");
    core.o.gate.allow = allow;
    await renderScreen(ApprovalDetailScreen, { params: { coreId: "c", approvalId: "a-1" } }, deps);
    await fireEvent.press(await screen.findByTestId("approval-approve"));
    return { client, core, resolves: () => resolves };
  }

  it("approve with biometric step-up retries once with the assertion", async () => {
    const { client, core, resolves } = await setup(true);
    await waitFor(() => expect(resolves()).toBe(2));
    expect(core.o.gate.prompts).toBe(1);
    const second = client.calls.filter((c) => c.method === "approvals.resolve")[1];
    expect(second?.params.stepUp).toBeTruthy();
    expect(await screen.findByText("Approved")).toBeTruthy();
  });

  it("biometric denied → no retry, approval stays pending", async () => {
    const { core, resolves } = await setup(false);
    expect(await screen.findByTestId("approval-error")).toHaveTextContent(/Biometric confirmation cancelled/);
    expect(resolves()).toBe(1);
    expect(core.o.gate.prompts).toBe(1);
    expect(screen.getByText("Pending")).toBeTruthy();
    expect(screen.getByTestId("approval-approve")).toBeTruthy();
  });
});

describe("Pairing", () => {
  const offer = JSON.stringify({ gatewayUrl: "http://gw", coreId: "core-1", pairingId: "p", code: "ABCD2345" });

  async function start() {
    const { core, deps } = makeDeps(null);
    const session = { coreId: "core-1", sas: "123456", waitForApproval: jest.fn() };
    const pair = jest.spyOn(core, "pair").mockResolvedValue(session);
    const complete = jest
      .spyOn(core, "completePairing")
      .mockResolvedValue({ record: { id: "core-1", coreId: "core-1" } as never, stepUpRegistered: true });
    const r = await renderScreen(PairingScreen, {}, deps);
    await fireEvent.changeText(screen.getByTestId("pairing-input"), `orkestr://pair?offer=${encodeURIComponent(offer)}`);
    await fireEvent.press(screen.getByTestId("pairing-submit"));
    return { pair, complete, navigation: r.navigation };
  }

  it("shows the SAS and completes only after the user confirms it", async () => {
    const { pair, complete, navigation } = await start();
    expect(await screen.findByTestId("pairing-sas")).toHaveTextContent("123 456");
    expect(pair).toHaveBeenCalledWith(expect.objectContaining({ coreId: "core-1", pairingId: "p" }));
    expect(complete).not.toHaveBeenCalled();
    await fireEvent.press(screen.getByTestId("pairing-sas-confirm"));
    await waitFor(() => expect(complete).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(navigation.reset).toHaveBeenCalled());
  });

  it("SAS mismatch aborts the pairing", async () => {
    const { complete } = await start();
    await fireEvent.press(await screen.findByTestId("pairing-sas-mismatch"));
    expect(complete).not.toHaveBeenCalled();
    expect(await screen.findByTestId("pairing-error")).toHaveTextContent(/Codes did not match/);
  });

  it("rejects input that is not an offer", async () => {
    const { core, deps } = makeDeps(null);
    const pair = jest.spyOn(core, "pair");
    await renderScreen(PairingScreen, {}, deps);
    await fireEvent.changeText(screen.getByTestId("pairing-input"), "hello");
    await fireEvent.press(screen.getByTestId("pairing-submit"));
    expect(await screen.findByTestId("pairing-error")).toHaveTextContent(/not a pairing code/);
    expect(pair).not.toHaveBeenCalled();
  });
});

describe("notification tap", () => {
  it("routes an approval notification to the approval detail", () => {
    const nav = { navigate: jest.fn() };
    handleNotificationTap(nav, { kind: "approval", coreId: "core-1", approvalId: "a-9" });
    expect(nav.navigate).toHaveBeenCalledWith("ApprovalDetail", { coreId: "core-1", approvalId: "a-9" });
    handleNotificationTap(nav, { category: "taskCompleted" });
    expect(nav.navigate).toHaveBeenLastCalledWith("Cores");
  });
});
