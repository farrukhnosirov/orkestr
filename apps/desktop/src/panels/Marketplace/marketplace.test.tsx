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
import type {
  EntryDetails,
  InstalledPackage,
  MarketEntry,
  PublisherKey,
} from "./api";
import {
  compareVersions,
  entryAction,
  fingerprintRows,
  initialDecisions,
  installGate,
} from "./model";

const FP =
  "ab12 cd34 ef56 0011 2233 4455 6677 8899 aabb ccdd eeff 0123 4567 89ab cdef 0f0f";

function entry(patch: Partial<MarketEntry> = {}): MarketEntry {
  return {
    id: "acme.linter",
    kind: "AgentTemplate",
    name: "Linter",
    version: "1.0.0",
    publisher: "acme",
    description: "Lints code",
    keyId: "k1",
    sha256: "00",
    url: "packages/x",
    revoked: false,
    tags: ["agent-template", "reviewer"],
    sourceId: "dir-1",
    ...patch,
  };
}

function key(
  trust: PublisherKey["trust"],
  patch: Partial<PublisherKey> = {},
): PublisherKey {
  return {
    keyId: "k1",
    publisher: "acme",
    publicKey: "pk",
    fingerprint: FP,
    trust,
    firstSeen: "2026-10-01T00:00:00Z",
    ...patch,
  };
}

function details(
  e: MarketEntry,
  trust: PublisherKey["trust"],
  permissions: EntryDetails["permissions"] = [],
): EntryDetails {
  return { entry: e, versions: [e.version], permissions, key: key(trust) };
}

describe("marketplace model", () => {
  it("compares versions numerically", () => {
    expect(compareVersions("1.10.0", "1.9.0")).toBe(1);
    expect(compareVersions("1.0.0", "1.0.0")).toBe(0);
    expect(compareVersions("1.0.0-beta", "1.0.0")).toBe(-1);
  });

  it("derives the entry action and install gate", () => {
    expect(entryAction(entry())).toBe("install");
    expect(entryAction(entry({ installedVersion: "0.9.0" }))).toBe("update");
    expect(entryAction(entry({ installedVersion: "1.0.0" }))).toBe("installed");
    expect(entryAction(entry({ revoked: true }))).toBe("blocked");
    expect(installGate(details(entry(), "Untrusted"))).toBe("untrusted");
    expect(installGate(details(entry(), "Official"))).toBeNull();
    expect(installGate(details(entry(), "Revoked"))).toBe("keyRevoked");
    expect(installGate(details(entry({ revoked: true }), "Trusted"))).toBe(
      "revoked",
    );
  });

  it("formats fingerprints and denies permissions by default", () => {
    expect(fingerprintRows(FP)).toHaveLength(2);
    expect(
      initialDecisions([
        { capability: "Network", scope: "api.example.com", reason: "r" },
      ]),
    ).toEqual([
      { capability: "Network", scope: "api.example.com", allow: false },
    ]);
  });
});

// ── Panel ───────────────────────────────────────────────

const calls: Array<{ method: string; params: Record<string, unknown> }> = [];
let handlers: Array<(ev: Event) => void> = [];
let entries: MarketEntry[] = [];
let detailsById: Record<string, EntryDetails> = {};
let installed: InstalledPackage[] = [];
let keys: PublisherKey[] = [];

const fakeClient = {
  call: vi.fn(async (method: string, params: Record<string, unknown>) => {
    calls.push({ method, params });
    switch (method) {
      case "marketplace.search":
        return { entries };
      case "marketplace.get":
        return detailsById[params.id as string];
      case "marketplace.trustKey":
        return { key: key("Trusted") };
      case "marketplace.untrustKey":
        return { key: key("Untrusted") };
      case "marketplace.install": {
        const d = detailsById[params.id as string]!;
        if (d.entry.kind === "Plugin") {
          return {
            kind: "Plugin",
            version: d.entry.version,
            plugin: { plugin: { id: "plug-1" } },
            pendingGrants: d.permissions,
            templateIds: [],
          };
        }
        return {
          kind: d.entry.kind,
          version: d.entry.version,
          pendingGrants: [],
          templateIds: ["t1"],
        };
      }
      case "plugins.grant":
        return {};
      case "marketplace.installed":
        return { packages: installed };
      case "marketplace.uninstall":
        return {};
      case "marketplace.sources":
        return {
          sources: [
            {
              id: "official",
              name: "Orkestr official",
              kind: "Official",
              location: "bundled",
              enabled: true,
              entries: 2,
            },
            {
              id: "http-1",
              name: "team",
              kind: "Http",
              location: "http://gw/registry",
              enabled: true,
              entries: 0,
              lastError: "signature: index signature does not verify",
            },
          ],
        };
      case "marketplace.addSource":
        return {
          source: {
            id: "http-2",
            name: "x",
            kind: "Http",
            location: params.location,
            enabled: true,
            entries: 0,
          },
        };
      case "marketplace.keys":
        return { keys, localPublisher: undefined };
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

const { default: MarketplacePanel } = await import("./index");

function renderPanel() {
  return render(
    <MarketplacePanel {...({} as Parameters<typeof MarketplacePanel>[0])} />,
  );
}

const methods = () => calls.map((c) => c.method);

beforeEach(() => {
  applyLanguagePreference("en");
  calls.length = 0;
  handlers = [];
  const tpl = entry();
  const plugin = entry({
    id: "acme.fetcher",
    name: "Fetcher",
    kind: "Plugin",
    tags: ["tool"],
  });
  entries = [tpl, plugin];
  detailsById = {
    "acme.linter": details(tpl, "Untrusted"),
    "acme.fetcher": details(plugin, "Trusted", [
      {
        capability: "Network",
        scope: "api.example.com",
        reason: "Calls the API",
      },
    ]),
  };
  installed = [];
  keys = [key("Untrusted")];
});

afterEach(() => cleanup());

describe("Marketplace panel", () => {
  it("browses, filters by kind and asks to trust an unknown publisher before installing", async () => {
    renderPanel();
    const list = await screen.findByRole("list", { name: "Packages" });
    await within(list).findByText("Fetcher");
    fireEvent.change(screen.getByLabelText("Package kind"), {
      target: { value: "Plugin" },
    });
    await waitFor(() =>
      expect(
        calls.some(
          (c) =>
            c.method === "marketplace.search" && c.params.kind === "Plugin",
        ),
      ).toBe(true),
    );

    fireEvent.click(within(list).getByText("Linter"));
    const install = await screen.findByRole("button", { name: "Install" });
    fireEvent.click(install);
    const dialog = await screen.findByRole("dialog", {
      name: "Trust this publisher key?",
    });
    expect(within(dialog).getByText(fingerprintRows(FP)[0]!)).toBeTruthy();
    expect(methods()).not.toContain("marketplace.install");
    fireEvent.click(
      within(dialog).getByRole("button", { name: "Trust and continue" }),
    );
    await waitFor(() => expect(methods()).toContain("marketplace.install"));
    const trust = calls.find((c) => c.method === "marketplace.trustKey")!;
    expect(trust.params).toEqual({ keyId: "k1", fingerprint: FP });
    expect(await screen.findByRole("status")).toHaveProperty(
      "textContent",
      expect.stringContaining("Linter 1.0.0 installed"),
    );
  });

  it("shows the permission prompt for plugin packages and sends the decisions", async () => {
    renderPanel();
    const list = await screen.findByRole("list", { name: "Packages" });
    fireEvent.click(await within(list).findByText("Fetcher"));
    fireEvent.click(await screen.findByRole("button", { name: "Install" }));
    const dialog = await screen.findByRole("dialog", { name: "Permissions" });
    expect(within(dialog).getByText("api.example.com")).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("checkbox"));
    fireEvent.click(
      within(dialog).getByRole("button", { name: "Save and enable" }),
    );
    await waitFor(() => expect(methods()).toContain("plugins.grant"));
    expect(calls.find((c) => c.method === "plugins.grant")!.params).toEqual({
      pluginId: "plug-1",
      decisions: [
        { capability: "Network", scope: "api.example.com", allow: true },
      ],
      enable: true,
    });
  });

  it("refuses revoked entries without calling install", async () => {
    entries = [entry({ revoked: true })];
    detailsById["acme.linter"] = details(entries[0]!, "Trusted");
    renderPanel();
    const list = await screen.findByRole("list", { name: "Packages" });
    fireEvent.click(await within(list).findByText("Linter"));
    expect(await screen.findByText(/revoked by its registry/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Install" })).toBeNull();
  });

  it("lists installed packages with updates and uninstalls", async () => {
    installed = [
      {
        id: "acme.linter",
        kind: "AgentTemplate",
        name: "Linter",
        version: "1.0.0",
        sourceId: "dir-1",
        keyId: "k1",
        sha256: "00",
        templateIds: ["t1"],
        revoked: false,
        updateAvailable: "1.1.0",
        installedAt: "2026-10-01T00:00:00Z",
      },
    ];
    renderPanel();
    fireEvent.click(screen.getByRole("tab", { name: /Installed/ }));
    expect(await screen.findByText("1.1.0 available")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /Uninstall/ }));
    await waitFor(() => expect(methods()).toContain("marketplace.uninstall"));
  });

  it("adds a gateway source with a token and shows refresh errors", async () => {
    renderPanel();
    fireEvent.click(screen.getByRole("tab", { name: /Sources/ }));
    expect(await screen.findByText(/Last refresh failed/)).toBeTruthy();
    fireEvent.change(screen.getByLabelText("Location"), {
      target: { value: "http://gw:8787/registry" },
    });
    fireEvent.change(screen.getByLabelText("Publish token"), {
      target: { value: "tok" },
    });
    fireEvent.click(screen.getByRole("button", { name: /Add source/ }));
    await waitFor(() => expect(methods()).toContain("marketplace.addSource"));
    expect(
      calls.find((c) => c.method === "marketplace.addSource")!.params,
    ).toMatchObject({
      kind: "Http",
      location: "http://gw:8787/registry",
      token: "tok",
    });
  });

  it("trusts keys from the Keys tab after showing the fingerprint and reloads on MarketplaceUpdated", async () => {
    renderPanel();
    fireEvent.click(screen.getByRole("tab", { name: /Keys/ }));
    fireEvent.click(await screen.findByRole("button", { name: "Trust…" }));
    const dialog = await screen.findByRole("dialog", {
      name: "Trust this publisher key?",
    });
    fireEvent.click(
      within(dialog).getByRole("button", { name: "Trust and continue" }),
    );
    await waitFor(() => expect(methods()).toContain("marketplace.trustKey"));
    const before = calls.filter((c) => c.method === "marketplace.keys").length;
    handlers.forEach((h) =>
      h({
        kind: "MarketplaceUpdated",
        payload: { change: "refreshed" },
      } as unknown as Event),
    );
    await waitFor(() =>
      expect(
        calls.filter((c) => c.method === "marketplace.keys").length,
      ).toBeGreaterThan(before),
    );
  });
});
