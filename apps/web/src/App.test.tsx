import { type DeviceRecord, MemoryKeyStore, OrkestrError, type PairingSession } from "@orkestr/client";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";

import { App } from "./App";
import { type AppDeps, DepsProvider } from "./deps";
import type { AuthState, GatewayApi } from "./gateway";
import { buildResources, i18n } from "./i18n";
import { EditorPanel } from "./panels/EditorPanel";
import { ApprovalsPanel } from "./panels/lists";
import { TerminalPanel } from "./panels/TerminalPanel";
import { type RemoteClient, callWithStepUp } from "./remote";
import { Pairing } from "./screens/Pairing";

type Handler = (params: Record<string, unknown>) => unknown;

function fakeClient(handlers: Record<string, Handler>) {
  const calls: { method: string; params: Record<string, unknown> }[] = [];
  const client = {
    call: vi.fn(async (method: string, params: Record<string, unknown>) => {
      calls.push({ method, params });
      const h = handlers[method];
      if (!h) throw new OrkestrError("not_found", method);
      return h(params);
    }),
    subscribe: vi.fn(() => () => {}),
    close: vi.fn(),
  } as unknown as RemoteClient;
  return { client, calls };
}

function fakeGateway(state: Partial<AuthState> = {}): GatewayApi {
  return {
    url: "http://gw",
    state: vi.fn(async () => ({ hasAccount: true, authenticated: false, totpEnabled: true, ...state })),
    registerPasskey: vi.fn(async () => ({ token: "t" })),
    loginPasskey: vi.fn(async () => ({ token: "t" })),
    totpLogin: vi.fn(async (code: string) => {
      if (code !== "123456") throw Object.assign(new Error("bad"), { status: 401 });
      return { token: "t" };
    }),
    logout: vi.fn(async () => {}),
    vapidKey: vi.fn(async () => null),
    enrollmentToken: vi.fn(async () => ({ token: "enroll-123", expiresAt: "2026-10-22T10:00:00Z" })),
  };
}

const device = {
  id: "core-1",
  gatewayUrl: "http://gw",
  coreId: "core-1",
  deviceId: "dev-1",
  corePubKey: "x",
  role: "operator",
  deviceName: "Browser",
  keyPair: {} as CryptoKeyPair,
  pairedAt: 1,
} satisfies DeviceRecord;

function deps(over: Partial<AppDeps> = {}): AppDeps {
  return {
    gateway: fakeGateway(),
    store: new MemoryKeyStore(),
    connect: vi.fn(async () => fakeClient({ "projects.list": () => ({ projects: [] }) }).client),
    pair: vi.fn(),
    stepUp: vi.fn(async () => ({ credentialId: "c", authenticatorData: "a", clientDataJSON: "d", signature: "s" })),
    registerStepUp: vi.fn(async () => ({})),
    ...over,
  };
}

function renderWith(d: AppDeps, ui: ReactNode) {
  return render(
    <I18nextProvider i18n={i18n}>
      <DepsProvider deps={d}>{ui}</DepsProvider>
    </I18nextProvider>,
  );
}

beforeAll(async () => {
  await i18n.changeLanguage("en");
});
afterEach(cleanup);

describe("locales", () => {
  it("ship en and ru with the same keys (deep)", () => {
    const res = buildResources();
    const keys = (o: unknown, p = ""): string[] =>
      typeof o === "object" && o !== null
        ? Object.entries(o).flatMap(([k, v]) => keys(v, `${p}${k}.`))
        : [p.replace(/_(one|few|many|other)\.$/, ".")];
    const en = new Set(keys(res.en?.common));
    const ru = new Set(keys(res.ru?.common));
    expect([...en].filter((k) => !ru.has(k))).toEqual([]);
    expect([...ru].filter((k) => !en.has(k))).toEqual([]);
  });
});

describe("login", () => {
  it("rejects a wrong TOTP code and continues to pairing on success", async () => {
    const d = deps();
    renderWith(d, <App />);
    const code = await screen.findByLabelText("Authenticator code");
    fireEvent.change(code, { target: { value: "000000" } });
    fireEvent.click(screen.getByRole("button", { name: "Sign in with code" }));
    expect((await screen.findByRole("alert")).textContent).toMatch(/rejected/);
    fireEvent.change(code, { target: { value: "123456" } });
    fireEvent.click(screen.getByRole("button", { name: "Sign in with code" }));
    expect(await screen.findByText("Pair this browser")).toBeTruthy();
  });

  it("offers passkey registration on a fresh gateway", async () => {
    const d = deps({ gateway: fakeGateway({ hasAccount: false }) });
    renderWith(d, <App />);
    fireEvent.click(await screen.findByRole("button", { name: "Create account with a passkey" }));
    await waitFor(() => expect(d.gateway.registerPasskey).toHaveBeenCalledWith("owner"));
  });

  it("goes straight to the workspace for a paired, signed-in device", async () => {
    const store = new MemoryKeyStore();
    await store.put(device);
    const d = deps({ store, gateway: fakeGateway({ authenticated: true }) });
    renderWith(d, <App />);
    expect(await screen.findByText("No projects yet.")).toBeTruthy();
    expect(d.connect).toHaveBeenCalledWith(device, undefined);
  });
});

describe("pairing", () => {
  it("shows the SAS and finishes when the desktop approves", async () => {
    let approve: (r: DeviceRecord) => void = () => {};
    const session: PairingSession = {
      coreId: "core-1",
      sas: "482913",
      waitForApproval: () => new Promise((r) => (approve = r)),
    };
    const pair = vi.fn(async () => session);
    const onPaired = vi.fn();
    renderWith(deps({ pair }), <Pairing onPaired={onPaired} />);
    fireEvent.change(screen.getByLabelText("Pairing text (orkestr-pair:…)"), {
      target: { value: 'orkestr-pair:{"gatewayUrl":"http://gw","coreId":"core-1","pairingId":"p1","code":"ABCD1234"}' },
    });
    fireEvent.click(screen.getByRole("button", { name: "Pair" }));
    expect((await screen.findByLabelText("Verification code")).textContent).toBe("482 913");
    expect(pair).toHaveBeenCalledWith(expect.objectContaining({ pairingId: "p1", code: "ABCD1234", coreId: "core-1" }));
    await act(async () => approve(device));
    expect(onPaired).toHaveBeenCalledWith(device);
  });

  it("creates a one-time enrollment token for connecting a computer", async () => {
    const d = deps();
    renderWith(d, <Pairing onPaired={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Create enrollment token" }));
    expect((await screen.findByLabelText("Enrollment token")).textContent).toBe("enroll-123");
    expect(d.gateway.enrollmentToken).toHaveBeenCalledOnce();
  });

  it("rejects text that is not a pairing payload", async () => {
    renderWith(deps(), <Pairing onPaired={() => {}} />);
    fireEvent.change(screen.getByLabelText("Pairing text (orkestr-pair:…)"), { target: { value: "hello" } });
    fireEvent.click(screen.getByRole("button", { name: "Pair" }));
    expect((await screen.findByRole("alert")).textContent).toMatch(/not a valid/);
  });
});

describe("approvals with step-up", () => {
  it("retries approvals.resolve with an assertion after forbidden{stepUpRequired}", async () => {
    let resolved = 0;
    const { client, calls } = fakeClient({
      "approvals.list": () => ({
        approvals: resolved
          ? []
          : [{ id: "a1", risk: "High", actionSummary: "git push --force", capability: "Git", status: "Pending" }],
      }),
      "approvals.resolve": (p) => {
        if (!p.stepUp) throw new OrkestrError("forbidden", "step-up", { stepUpRequired: true, challenge: "Y2hhbA" });
        resolved++;
        return { approval: {} };
      },
    });
    const d = deps();
    renderWith(d, <ApprovalsPanel client={client} />);
    fireEvent.click(await screen.findByRole("button", { name: "Approve" }));
    expect(await screen.findByText("Nothing is waiting for your approval.")).toBeTruthy();
    expect(d.stepUp).toHaveBeenCalledWith("Y2hhbA");
    const resolves = calls.filter((c) => c.method === "approvals.resolve");
    expect(resolves).toHaveLength(2);
    expect(resolves[1]?.params).toMatchObject({ approvalId: "a1", decision: "Approve", stepUp: { clientDataJSON: "d" } });
  });

  it("does not retry on other errors", async () => {
    const { client } = fakeClient({ "approvals.resolve": () => { throw new OrkestrError("forbidden", "no role"); } });
    const stepUp = vi.fn();
    await expect(
      callWithStepUp(client, "approvals.resolve", { approvalId: "a", decision: "Reject" }, stepUp),
    ).rejects.toMatchObject({ code: "forbidden" });
    expect(stepUp).not.toHaveBeenCalled();
  });
});

describe("terminal and editor", () => {
  it("shows 'remote terminal disabled' on forbidden", async () => {
    const { client } = fakeClient({ "terminal.open": () => { throw new OrkestrError("forbidden", "remote terminal is not enabled"); } });
    renderWith(deps(), <TerminalPanel client={client} projectId="p1" />);
    fireEvent.click(screen.getByRole("button", { name: "Open terminal" }));
    expect(await screen.findByText("Remote terminal disabled")).toBeTruthy();
  });

  it("reads and saves a file with the expected hash", async () => {
    const { client, calls } = fakeClient({
      "files.read": () => ({ path: "README.md", content: "hello", binary: false, hash: "h1", readonly: false }),
      "files.write": () => ({ hash: "h2" }),
    });
    renderWith(deps(), <EditorPanel client={client} projectId="p1" />);
    fireEvent.change(screen.getByLabelText("File path in the project"), { target: { value: "README.md" } });
    fireEvent.click(screen.getByRole("button", { name: "Open" }));
    const area = await screen.findByLabelText("File content");
    expect((area as HTMLTextAreaElement).value).toBe("hello");
    fireEvent.change(area, { target: { value: "hello orkestr-p4" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByText("Saved.")).toBeTruthy();
    expect(calls.find((c) => c.method === "files.write")?.params).toEqual({
      projectId: "p1",
      path: "README.md",
      content: "hello orkestr-p4",
      expectedHash: "h1",
    });
  });
});
