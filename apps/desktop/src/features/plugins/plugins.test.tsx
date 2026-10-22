import type { Event } from "@orkestr/protocol";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { applyLanguagePreference } from "../../i18n";
import type { PluginInfo } from "./api";
import { pendingCount, sortPlugins, stateTone, toggleAction } from "./model";

const FP =
  "ab12 cd34 ef56 0011 2233 4455 6677 8899 aabb ccdd eeff 0123 4567 89ab cdef 0f0f";

function info(patch: {
  id?: string;
  name?: string;
  state?: PluginInfo["plugin"]["state"];
  grants?: PluginInfo["grants"];
  pending?: PluginInfo["pendingGrants"];
  signer?: string;
} = {}): PluginInfo {
  const id = patch.id ?? "p1";
  const name = patch.name ?? "Hello widget";
  const state = patch.state ?? "Enabled";
  return {
    plugin: {
      id,
      packageId: `dev.${id}`,
      state,
      name,
      version: "0.1.0",
      publisher: "dev",
      kinds: ["Widget", "Tool"],
      permissions: ["Network"],
      enabled: state === "Enabled",
      source: "file",
      installedAt: "2026-10-01T00:00:00Z",
    },
    manifest: {
      id: `dev.${id}`,
      name,
      version: "0.1.0",
      publisher: "dev",
      description: "Says hello",
      abi: 1,
      kinds: ["Widget", "Tool"],
      permissions: [],
      tools: [],
      widgets: [],
      deployProviders: [],
      agentTemplates: [],
      subscribes: [],
      limits: {} as PluginInfo["manifest"]["limits"],
    },
    grants: patch.grants ?? [],
    pendingGrants: patch.pending ?? [],
    signerFingerprint: patch.signer,
    sha256: "00",
  };
}

describe("plugins model", () => {
  it("maps states, toggles and ordering", () => {
    expect(stateTone("Enabled")).toBe("success");
    expect(stateTone("Killed")).toBe("danger");
    expect(stateTone("Quarantined")).toBe("warning");
    expect(toggleAction(info())).toBe("disable");
    expect(toggleAction(info({ state: "Disabled" }))).toBe("enable");
    const pending = info({
      id: "p2",
      name: "Zed",
      state: "Installed",
      pending: [{ capability: "Network", scope: "a.example", reason: "r" }],
    });
    expect(toggleAction(pending)).toBe("decide");
    expect(sortPlugins([info(), pending])[0]!.plugin.id).toBe("p2");
    expect(pendingCount([info(), pending])).toBe(1);
  });
});

// ── Component ───────────────────────────────────────────

const calls: Array<{ method: string; params: Record<string, unknown> }> = [];
let handlers: Array<(ev: Event) => void> = [];
let plugins: PluginInfo[] = [];
let safeMode = false;

const fakeClient = {
  call: vi.fn(async (method: string, params: Record<string, unknown>) => {
    calls.push({ method, params });
    switch (method) {
      case "plugins.list":
        return { plugins, safeMode };
      case "plugins.setSafeMode":
        safeMode = params.enabled as boolean;
        return { safeMode };
      case "plugins.killAll":
        plugins = plugins.map((p) => ({
          ...p,
          plugin: { ...p.plugin, state: "Killed", enabled: false },
        }));
        return { killed: 2, callsInterrupted: 1 };
      case "plugins.enable":
      case "plugins.disable":
      case "plugins.grant":
        return { plugin: plugins[0] };
      case "plugins.uninstall":
        return {};
      case "plugins.logs":
        return {
          lines: [
            { ts: "2026-10-01T00:00:00Z", level: "info", message: "hello from wasm" },
            { ts: "2026-10-01T00:00:01Z", level: "warn", message: "slow query" },
          ],
        };
      case "plugins.install": {
        const p = info({
          id: "p9",
          name: "Fetcher",
          state: "Installed",
          pending: [{ capability: "Network", scope: "api.example.com", reason: "Calls the API" }],
        });
        return { plugin: p, pendingGrants: p.pendingGrants };
      }
      default:
        throw new Error(`unexpected ${method}`);
    }
  }),
  subscribe: vi.fn((_topics: string[], h: (ev: Event) => void) => {
    handlers.push(h);
    return () => {
      handlers = handlers.filter((x) => x !== h);
    };
  }),
};

vi.mock("../../core/client", () => ({
  maybeClient: () => fakeClient,
  getClient: () => fakeClient,
}));

const { PluginsManager } = await import("./PluginsManager");

const methods = () => calls.map((c) => c.method);
const paramsOf = (m: string) => calls.filter((c) => c.method === m).at(-1)!.params;

beforeEach(() => {
  applyLanguagePreference("en");
  calls.length = 0;
  handlers = [];
  safeMode = false;
  plugins = [
    info({
      signer: FP,
      grants: [
        { capability: "Network", scope: "api.example.com", allowed: true, decidedAt: "2026-10-01T00:00:00Z" },
      ],
    }),
    info({ id: "p2", name: "Deployer", state: "Disabled" }),
  ];
});

afterEach(() => cleanup());

describe("PluginsManager", () => {
  it("lists plugins with state, kinds, signer fingerprint and grants", async () => {
    render(<PluginsManager />);
    const list = await screen.findByRole("list", { name: "Installed plugins" });
    const rows = within(list).getAllByRole("listitem", { hidden: false });
    expect(rows.length).toBeGreaterThanOrEqual(2);
    expect(within(list).getByText("Enabled")).toBeTruthy();
    expect(within(list).getByText("Disabled")).toBeTruthy();
    expect(within(list).getAllByText("Widget").length).toBe(2);
    expect(screen.getByText(FP.split(" ").slice(0, 8).join(" "))).toBeTruthy();
    expect(within(list).getByText("Unsigned dev package")).toBeTruthy();
    expect(within(list).getByText("api.example.com")).toBeTruthy();
  });

  it("revokes a grant via plugins.grant deny and enables / disables", async () => {
    render(<PluginsManager />);
    fireEvent.click(
      await screen.findByRole("button", { name: "Revoke Network for Hello widget" }),
    );
    await waitFor(() => expect(methods()).toContain("plugins.grant"));
    expect(paramsOf("plugins.grant")).toEqual({
      pluginId: "p1",
      decisions: [{ capability: "Network", scope: "api.example.com", allow: false }],
      enable: false,
    });
    fireEvent.click(screen.getByRole("button", { name: "Disable" }));
    await waitFor(() => expect(methods()).toContain("plugins.disable"));
    fireEvent.click(screen.getByRole("button", { name: "Enable" }));
    await waitFor(() => expect(methods()).toContain("plugins.enable"));
    expect(paramsOf("plugins.enable")).toMatchObject({ pluginId: "p2" });
  });

  it("asks before killing all plugins and reports the counts", async () => {
    render(<PluginsManager />);
    fireEvent.click(await screen.findByRole("button", { name: "Kill all plugins" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Kill all plugins?" });
    expect(methods()).not.toContain("plugins.killAll");
    fireEvent.click(within(dialog).getByRole("button", { name: "Kill all plugins" }));
    await waitFor(() => expect(methods()).toContain("plugins.killAll"));
    expect(await screen.findByText(/Killed 2 plugins, 1 call/)).toBeTruthy();
    expect((await screen.findAllByText("Killed")).length).toBe(2);
  });

  it("toggles safe mode and explains the effect", async () => {
    render(<PluginsManager />);
    expect(await screen.findByText(/no plugin code is loaded or run/)).toBeTruthy();
    fireEvent.click(screen.getByRole("switch", { name: /Safe mode/ }));
    await waitFor(() => expect(methods()).toContain("plugins.setSafeMode"));
    expect(paramsOf("plugins.setSafeMode")).toEqual({ enabled: true });
    expect(await screen.findByText(/Safe mode is on/)).toBeTruthy();
    expect(
      (screen.getByRole("button", { name: "Enable" }) as HTMLButtonElement).disabled,
    ).toBe(true);
  });

  it("confirms uninstall and shows plugin logs", async () => {
    render(<PluginsManager />);
    fireEvent.click(await screen.findByRole("button", { name: "Logs of Hello widget" }));
    const logs = await screen.findByRole("dialog", { name: "Logs: Hello widget" });
    expect(await within(logs).findByText("hello from wasm")).toBeTruthy();
    fireEvent.click(within(logs).getAllByRole("button", { name: "Close" }).at(-1)!);
    fireEvent.click(screen.getByRole("button", { name: "Uninstall Deployer" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Uninstall Deployer?" });
    expect(methods()).not.toContain("plugins.uninstall");
    fireEvent.click(within(dialog).getByRole("button", { name: "Uninstall" }));
    await waitFor(() => expect(methods()).toContain("plugins.uninstall"));
    expect(paramsOf("plugins.uninstall")).toEqual({ pluginId: "p2" });
  });

  it("installs from a file path and prompts for pending grants", async () => {
    render(<PluginsManager />);
    fireEvent.change(await screen.findByRole("textbox", { name: "Install from file" }), {
      target: { value: "C:\\pkgs\\fetcher.orkp" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Install" }));
    await waitFor(() => expect(methods()).toContain("plugins.install"));
    expect(paramsOf("plugins.install")).toEqual({
      path: "C:\\pkgs\\fetcher.orkp",
      source: "file",
    });
    const dialog = await screen.findByRole("dialog", { name: "Permissions" });
    fireEvent.click(within(dialog).getByRole("checkbox"));
    fireEvent.click(within(dialog).getByRole("button", { name: "Save and enable" }));
    await waitFor(() =>
      expect(paramsOf("plugins.grant")).toEqual({
        pluginId: "p9",
        decisions: [{ capability: "Network", scope: "api.example.com", allow: true }],
        enable: true,
      }),
    );
  });

  it("reloads on PluginStateChanged", async () => {
    render(<PluginsManager />);
    await screen.findByRole("list", { name: "Installed plugins" });
    const before = methods().filter((m) => m === "plugins.list").length;
    handlers.forEach((h) =>
      h({ kind: "PluginStateChanged", payload: {} } as unknown as Event),
    );
    await waitFor(() =>
      expect(methods().filter((m) => m === "plugins.list").length).toBeGreaterThan(before),
    );
  });

  it("renders in Russian", async () => {
    applyLanguagePreference("ru");
    render(<PluginsManager />);
    expect(await screen.findByRole("button", { name: "Остановить все плагины" })).toBeTruthy();
    expect(screen.getByText("Безопасный режим")).toBeTruthy();
    expect(await screen.findByText("Включён")).toBeTruthy();
  });
});
