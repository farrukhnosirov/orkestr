// Settings → Remote & Sync (ADR-0031): configure, Sync now, Seamless
// Continue (publish + resume restores editors), conflicts, and a graceful
// notice while remote.* answers unimplementedFeature.
import type { Project } from "@orkestr/protocol";
import { OrkestrError } from "@orkestr/client";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const calls: { method: string; params: unknown }[] = [];
let configured = false;
let conflicts: { id: string; projectId: string; path: string; conflictCopy?: string; detectedAt: string }[] = [];
let remoteImplemented = false;

const unimplemented = (m: string) =>
  new OrkestrError("bad_request", `\`${m}\` is not implemented yet (unimplemented-feature)`, { unimplementedFeature: m });

vi.mock("../../core/client", () => ({
  getClient: () => ({
    call: (method: string, params: unknown) => {
      calls.push({ method, params });
      switch (method) {
        case "sync.status":
          return Promise.resolve({ projectId: "p1", configured, pending: 0, conflicts: conflicts.length });
        case "sync.listConflicts":
          return Promise.resolve({ conflicts });
        case "sync.configure":
          configured = true;
          return Promise.resolve({ projectId: "p1", configured: true, pending: 0, conflicts: 0, generatedRemoteKey: "KEY-123" });
        case "sync.now":
          return Promise.resolve({ pushed: 3, pulled: 1, bytes: 10, conflicts: 0 });
        case "sync.continueOn":
          return Promise.resolve({ publishedAt: "2026-10-22T10:00:00Z", report: {}, summary: "1 task(s): Backend API" });
        case "sync.resume":
          return Promise.resolve({
            report: { pushed: 0, pulled: 2, bytes: 0, conflicts: 0 },
            uiState: { openEditors: [{ path: "src/a.rs" }], activePath: "src/a.rs" },
            summary: "1 task(s): Backend API",
          });
        case "sync.resolveConflict":
          conflicts = [];
          return Promise.resolve({});
        case "remote.status":
          return remoteImplemented
            ? Promise.resolve({ enabled: true, connected: true, sessions: 1 })
            : Promise.reject(unimplemented(method));
        case "devices.list":
          return Promise.resolve({
            devices: [
              { deviceId: "d1", name: "Phone", role: "operator", remoteTerminal: false, createdAt: "2026-10-01T00:00:00Z", revoked: false },
            ],
          });
        case "devices.revoke":
          return Promise.resolve({});
        default:
          return Promise.reject(new Error(`unexpected ${method}`));
      }
    },
  }),
  maybeClient: () => null,
}));

import { i18n } from "../../i18n";
import { useAppStore } from "../../store/app";
import { useEditorStore } from "../../store/editor";
import { RemoteSyncSettings } from "./RemoteSyncSettings";

const project = (sync: "Allowed" | "NoRemoteSync" = "Allowed"): Project =>
  ({
    id: "p1",
    name: "Payroll",
    slug: "payroll",
    description: "",
    rootPath: "C:/work/payroll",
    repositoryId: null,
    privacy: { ai: { mode: "CloudAllowed" }, sync },
    classification: "Internal",
    approvalMode: "Ask",
    budget: null,
    color: null,
    icon: null,
    tags: [],
    archived: false,
    createdAt: "2026-01-01T00:00:00Z",
    updatedAt: "2026-01-01T00:00:00Z",
    lastOpenedAt: null,
  }) as Project;

beforeEach(async () => {
  await i18n.changeLanguage("en");
  calls.length = 0;
  configured = false;
  conflicts = [];
  remoteImplemented = false;
  useAppStore.setState({ projects: [project()], activeProjectId: "p1" });
});

afterEach(cleanup);

describe("Remote & Sync settings", () => {
  it("configures sync and shows the generated key once", async () => {
    render(<RemoteSyncSettings projectId="p1" />);
    expect(await screen.findByText("Not configured")).toBeTruthy();
    fireEvent.change(screen.getByLabelText("Gateway URL"), { target: { value: "http://127.0.0.1:8787" } });
    fireEvent.change(screen.getByLabelText("Access token"), { target: { value: "tok" } });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Save" }));
    });
    expect(await screen.findByText("KEY-123")).toBeTruthy();
    const cfg = calls.find((c) => c.method === "sync.configure");
    expect(cfg?.params).toEqual({ projectId: "p1", serverUrl: "http://127.0.0.1:8787", accessToken: "tok" });
    expect(await screen.findByText("Configured")).toBeTruthy();
  });

  it("syncs now, continues on another device and restores editors on resume", async () => {
    configured = true;
    useEditorStore.setState({ tabs: [], activePath: null });
    render(<RemoteSyncSettings projectId="p1" />);
    expect(await screen.findByText("Configured")).toBeTruthy();
    await act(async () => {
      fireEvent.click(await screen.findByRole("button", { name: "Sync now" }));
    });
    expect(await screen.findByText("Synced: 3 up, 1 down, conflicts: 0.")).toBeTruthy();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Continue on another device" }));
    });
    expect(await screen.findByText(/Ready to continue elsewhere\. 1 task\(s\): Backend API/)).toBeTruthy();
    expect(calls.find((c) => c.method === "sync.continueOn")?.params).toEqual({
      projectId: "p1",
      uiState: { openEditors: [], activePath: null },
    });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Resume from another device" }));
    });
    expect(await screen.findByText(/Restored the last session/)).toBeTruthy();
    await waitFor(() => expect(useEditorStore.getState().tabs.map((t) => t.path)).toContain("src/a.rs"));
  });

  it("lists conflicts and resolves them", async () => {
    configured = true;
    conflicts = [{ id: "c1", projectId: "p1", path: "doc.txt", conflictCopy: "doc.conflict-dB-1234.txt", detectedAt: "2026-10-22T10:00:00Z" }];
    render(<RemoteSyncSettings projectId="p1" />);
    expect(await screen.findByText("doc.txt")).toBeTruthy();
    expect(screen.getByText("Other version saved as doc.conflict-dB-1234.txt")).toBeTruthy();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Take theirs" }));
    });
    expect(calls.find((c) => c.method === "sync.resolveConflict")?.params).toEqual({
      projectId: "p1",
      conflictId: "c1",
      resolution: "keepRemote",
    });
    expect(await screen.findByText("No conflicts.")).toBeTruthy();
  });

  it("explains a privacy policy that forbids sync", async () => {
    useAppStore.setState({ projects: [project("NoRemoteSync")], activeProjectId: "p1" });
    render(<RemoteSyncSettings projectId="p1" />);
    expect((await screen.findAllByText("The project’s privacy policy forbids remote sync.")).length).toBeGreaterThan(0);
    expect(screen.queryByLabelText("Gateway URL")).toBeNull();
  });

  it("shows a graceful notice while remote access is unimplemented, devices once it exists", async () => {
    render(<RemoteSyncSettings projectId="p1" />);
    expect(await screen.findByText("Remote access is not available in this build of the core yet.")).toBeTruthy();
    cleanup();
    remoteImplemented = true;
    render(<RemoteSyncSettings projectId="p1" />);
    expect(await screen.findByText("Phone")).toBeTruthy();
    expect(screen.getByText("Connected")).toBeTruthy();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Revoke" }));
    });
    expect(calls.find((c) => c.method === "devices.revoke")?.params).toEqual({ deviceId: "d1" });
  });

  it("is translated to Russian", async () => {
    await i18n.changeLanguage("ru");
    render(<RemoteSyncSettings projectId="p1" />);
    expect(await screen.findByText("Не настроена")).toBeTruthy();
    expect(screen.getByText("Зашифрованная синхронизация")).toBeTruthy();
  });
});
