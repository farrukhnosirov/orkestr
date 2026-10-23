// Settings → Updates, the title-bar badge and the update store (ADR-0048).
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { UpdateStatus } from "./api";

const calls: Array<[string, unknown]> = [];
let reply: (cmd: string, args: unknown) => unknown = () => null;

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: unknown) => {
    calls.push([cmd, args]);
    try {
      return Promise.resolve(reply(cmd, args));
    } catch (e) {
      return Promise.reject(e instanceof Error ? e : new Error(String(e)));
    }
  },
  isTauri: () => true,
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));
vi.mock("../../lib/tauri", () => ({
  isMainWindow: () => true,
  isTauri: () => true,
  canPickPaths: () => false,
  pickFile: () => Promise.resolve(null),
  pickFolder: () => Promise.resolve(null),
}));

import { describeBootError } from "./bootError";
import { parseUpdateError } from "./api";
import { hasUpdateBadge, useUpdatesStore } from "./store";
import { UpdateBadge } from "./UpdateBadge";
import { UpdatesSection } from "./UpdatesSection";
import { useSettingsStore } from "../settings/settingsStore";

function status(over: Partial<UpdateStatus> = {}): UpdateStatus {
  return {
    currentVersion: "0.1.0",
    phase: "idle",
    settings: {
      remoteEnabled: false,
      endpoint: "https://updates.orkestr.dev/{{target}}/{{arch}}/{{current_version}}",
      channel: "stable",
      checkOnStartup: true,
      checkIntervalHours: 24,
      autoDownload: false,
      localPath: null,
      backupRetention: 5,
    },
    available: null,
    progress: null,
    lastCheckedAt: null,
    latestSeen: null,
    lastError: null,
    installOnExit: false,
    startup: null,
    backupsDir: "C:\\Users\\me\\AppData\\Roaming\\Orkestr\\Orkestr\\data\\backups",
    backups: [],
    ...over,
  };
}

const offer = (signature: string | null = "c2ln"): UpdateStatus["available"] => ({
  version: "0.1.1",
  currentVersion: "0.1.0",
  notes: "## What's new\n- Updates in Settings",
  notesI18n: { ru: "## Что нового\n- Обновления в настройках" },
  pubDate: "2026-10-23T12:00:00Z",
  platformKey: "windows-x86_64",
  location: { kind: "url", value: "https://updates.orkestr.dev/Orkestr_0.1.1_x64-setup.exe" },
  signature,
  source: "remote",
  fileName: "Orkestr_0.1.1_x64-setup.exe",
  size: 12_000_000,
  prerelease: false,
});

beforeEach(() => {
  calls.length = 0;
  reply = () => null;
  useUpdatesStore.setState({ status: null, busy: false, error: null });
});
afterEach(cleanup);

describe("update store", () => {
  it("checks through the shell and applies the returned status", async () => {
    reply = (cmd) => (cmd === "updates_check" ? status({ phase: "upToDate", lastCheckedAt: new Date().toISOString() }) : null);
    await useUpdatesStore.getState().check();
    expect(calls).toEqual([["updates_check", { localPath: null }]]);
    expect(useUpdatesStore.getState().status?.phase).toBe("upToDate");
  });

  it("saves settings as a full object and keeps command errors by code", async () => {
    useUpdatesStore.setState({ status: status() });
    reply = () => {
      throw new Error(JSON.stringify({ code: "invalid_endpoint", message: "http://x: the scheme must be https" }));
    };
    const ok = await useUpdatesStore.getState().saveSettings({ endpoint: "http://x" });
    expect(ok).toBe(false);
    const [cmd, args] = calls[0] ?? [];
    expect(cmd).toBe("updates_set_settings");
    expect((args as { settings: { endpoint: string; channel: string } }).settings).toMatchObject({ endpoint: "http://x", channel: "stable" });
    expect(useUpdatesStore.getState().error?.code).toBe("invalid_endpoint");
  });

  it("parses command errors and shows the badge only for offers", () => {
    expect(parseUpdateError(new Error('{"code":"offline","message":"dns"}'))).toEqual({ code: "offline", message: "dns" });
    expect(parseUpdateError("boom")).toEqual({ code: "unknown", message: "boom" });
    expect(hasUpdateBadge(status())).toBe(false);
    expect(hasUpdateBadge(status({ phase: "available", available: offer() }))).toBe(true);
    expect(hasUpdateBadge(status({ phase: "ready", available: offer() }))).toBe(true);
    expect(hasUpdateBadge(status({ phase: "upToDate" }))).toBe(false);
  });
});

describe("Settings → Updates", () => {
  it("shows the version, the not-configured hint and a manual check", async () => {
    useUpdatesStore.setState({ status: status() });
    reply = () => status({ phase: "error", lastError: { code: "not_configured", message: "no update source is configured" } });
    render(<UpdatesSection projectId={null} />);
    expect(screen.getByText("Orkestr 0.1.0")).toBeTruthy();
    expect(screen.getByText(/No update source yet/)).toBeTruthy();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
    });
    expect(calls.some(([c]) => c === "updates_check")).toBe(true);
    expect(screen.getByText(/No update source is set up/)).toBeTruthy();
  });

  it("offers a signed update with release notes and downloads it", async () => {
    useUpdatesStore.setState({ status: status({ phase: "available", available: offer() }) });
    reply = () => status({ phase: "downloading", available: offer(), progress: { downloaded: 6_000_000, total: 12_000_000 } });
    render(<UpdatesSection projectId={null} />);
    expect(screen.getByText("Version 0.1.1 is available")).toBeTruthy();
    expect(screen.getByText("Updates in Settings")).toBeTruthy();
    expect(screen.getByText("Signed")).toBeTruthy();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Download" }));
    });
    expect(calls).toContainEqual(["updates_download", { allowUnsigned: false }]);
    expect(screen.getByRole("progressbar").getAttribute("aria-valuenow")).toBe("50");
  });

  it("requires consent for an unsigned local installer", async () => {
    const unsigned = { ...offer(null)!, source: "localInstaller" as const, location: { kind: "file" as const, value: "D:\\Orkestr_0.1.1_x64-setup.exe" } };
    useUpdatesStore.setState({ status: status({ phase: "available", available: unsigned }) });
    render(<UpdatesSection projectId={null} />);
    const prepare = screen.getByRole("button", { name: "Verify and prepare" }) as HTMLButtonElement;
    expect(prepare.disabled).toBe(true);
    fireEvent.click(screen.getByRole("checkbox"));
    expect(prepare.disabled).toBe(false);
    await act(async () => {
      fireEvent.click(prepare);
    });
    expect(calls).toContainEqual(["updates_download", { allowUnsigned: true }]);
  });

  it("installs now or when Orkestr closes once ready", async () => {
    useUpdatesStore.setState({ status: status({ phase: "ready", available: offer() }) });
    reply = () => status({ phase: "ready", available: offer(), installOnExit: true });
    render(<UpdatesSection projectId={null} />);
    expect(screen.getByRole("button", { name: "Install now and restart" })).toBeTruthy();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Install when Orkestr closes" }));
    });
    expect(calls).toContainEqual(["updates_install", { when: "onExit" }]);
    expect(screen.getByText("The update will be installed when you quit Orkestr.")).toBeTruthy();
  });

  it("lists backups and explains rollback after an upgrade", () => {
    useUpdatesStore.setState({
      status: status({
        startup: {
          version: "0.1.1",
          start: { kind: "upgraded", from: "0.1.0" },
          previous: { version: "0.1.0", previousVersion: null, since: "2026-10-22T10:00:00Z" },
          schema: { state: "compatible", pending: 1 },
          backup: null,
          backupError: null,
        },
        currentVersion: "0.1.1",
        backups: [
          {
            path: "C:\\b\\orkestr-backup-1.zip",
            reason: "preMigration",
            createdAt: "2026-10-23T12:00:00Z",
            fromVersion: "0.1.0",
            toVersion: "0.1.1",
            databases: ["orkestr.db"],
            files: 2,
            sizeBytes: 2048,
          },
        ],
      }),
    });
    render(<UpdatesSection projectId={null} />);
    expect(screen.getByText(/Updated from 0.1.0 to 0.1.1/)).toBeTruthy();
    expect(screen.getByText("Before data upgrade")).toBeTruthy();
    expect(screen.getByText("0.1.0 → 0.1.1")).toBeTruthy();
    expect(screen.getByText(/install the previous version's installer/)).toBeTruthy();
  });

  it("shows offline errors in plain words", () => {
    useUpdatesStore.setState({ status: status({ phase: "error", lastError: { code: "offline", message: "error sending request" } }) });
    render(<UpdatesSection projectId={null} />);
    expect(screen.getByText(/can't be reached/)).toBeTruthy();
  });
});

describe("title-bar badge and boot errors", () => {
  it("appears for an offer and opens Settings → Updates", () => {
    const { container, rerender } = render(<UpdateBadge />);
    expect(container.innerHTML).toBe("");
    act(() => useUpdatesStore.setState({ status: status({ phase: "available", available: offer() }) }));
    rerender(<UpdateBadge />);
    const badge = screen.getByRole("button", { name: "Update 0.1.1 is available" });
    fireEvent.click(badge);
    expect(useSettingsStore.getState().open).toBe(true);
    expect(useSettingsStore.getState().section).toBe("updates");
  });

  it("explains the downgrade guard", () => {
    expect(describeBootError("database_newer: the Orkestr data was written by a newer version (Orkestr 0.2.0)")).toMatch(
      /older than the one that last used your data.*Orkestr 0\.2\.0/,
    );
    expect(describeBootError("timed out")).toBeNull();
    expect(describeBootError(null)).toBeNull();
  });
});
