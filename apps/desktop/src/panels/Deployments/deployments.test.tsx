// Deployments panel (ADR-0041): environments list / create with vault
// credentials, adapter availability, plan preview → deploy, approval state,
// log tail, rollback and delete confirmations. The core client is mocked.
import type { AdapterInfo, Deployment, EnvironmentRecord } from "@orkestr/protocol";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { formatKv, parseKv, schemaFields, statusTone, targetKey } from "./logic";

const calls: { method: string; params: unknown }[] = [];

const adapters: AdapterInfo[] = [
  {
    id: "staticSite",
    target: "StaticSite",
    availability: { state: "Ready" },
    verification: "live",
    settingsSchema: { type: "object", required: ["port"], properties: { port: { type: "string" }, outputDir: { type: "string", default: "dist" } } },
  },
  {
    id: "vercel",
    target: "Vercel",
    availability: { state: "NeedsCredential", secret: "VERCEL_TOKEN" },
    verification: "requestShape",
    settingsSchema: { type: "object", required: ["project"], properties: { project: { type: "string" } } },
  },
];

const T = "2026-10-22T10:00:00Z";
const prod: EnvironmentRecord = {
  environment: {
    id: "env-prod",
    projectId: "p1",
    name: "production",
    kind: "Production",
    variables: {},
    secretRefs: [],
    urls: [],
    protected: true,
    permissions: [],
    createdAt: T,
    updatedAt: T,
  },
  target: "StaticSite",
  adapterSettings: { port: "8123" },
};

function dep(id: string, status: Deployment["status"], version: string): Deployment {
  return {
    id,
    projectId: "p1",
    environmentId: "env-prod",
    target: "StaticSite",
    status,
    version,
    taskId: null,
    triggeredBy: { type: "System" },
    url: status === "Succeeded" ? "http://127.0.0.1:8123/" : null,
    logObjectId: null,
    startedAt: T,
    finishedAt: null,
    createdAt: T,
  } as Deployment;
}

let history: Deployment[] = [];

vi.mock("../../core/client", () => ({
  getClient: () => ({
    call: (method: string, params: unknown) => {
      calls.push({ method, params });
      switch (method) {
        case "environments.list":
          return Promise.resolve({ environments: [prod] });
        case "deploy.adapters":
          return Promise.resolve({ adapters });
        case "secrets.list":
          return Promise.resolve({ secrets: [{ id: "sec-1", name: "APP_SECRET", scope: { type: "Global" } }] });
        case "deploy.list":
          return Promise.resolve({ deployments: history });
        case "deploy.get":
          return Promise.resolve({ deployment: history[0], approvalId: "ap-1" });
        case "deploy.logs":
          return Promise.resolve({ lines: ["[orkestr] deploy v2 → production", "copied 3 files"], nextOffset: 2, complete: true });
        case "deploy.plan":
          return Promise.resolve({
            plan: {
              adapterId: "staticSite",
              target: "StaticSite",
              environmentId: "env-prod",
              version: "v2",
              requiresApproval: true,
              steps: ["Copy `dist` to versions/v2"],
              requests: [{ method: "GET", target: "http://127.0.0.1:8123/", body: "Authorization: Bearer [REDACTED]" }],
              warnings: [],
            },
          });
        case "deploy.start": {
          history = [dep("d2", "AwaitingApproval", "v2"), ...history];
          return Promise.resolve({ deployment: history[0], approvalId: "ap-1" });
        }
        case "deploy.rollback":
          return Promise.resolve({ deployment: dep("d3", "Running", "v1") });
        case "approvals.resolve":
        case "environments.create":
        case "environments.delete":
        case "deploy.cancel":
          return Promise.resolve({ environment: prod, deployment: history[0] });
        default:
          return Promise.reject(new Error(`unexpected ${method}`));
      }
    },
  }),
  maybeClient: () => null,
}));

import { i18n } from "../../i18n";
import DeploymentsPanel from "./index";

const lastCall = (m: string) => [...calls].reverse().find((c) => c.method === m)?.params as Record<string, unknown> | undefined;

beforeEach(async () => {
  await i18n.changeLanguage("en");
  calls.length = 0;
  history = [dep("d1", "Succeeded", "v1")];
});
afterEach(cleanup);

describe("deployments logic", () => {
  it("parses KEY=VALUE, schema fields, targets and tones", () => {
    expect(parseKv("A=1\n# c\n\nB = two=2\nbad")).toEqual({ A: "1", B: "two=2" });
    expect(formatKv({ A: "1", B: "2" })).toBe("A=1\nB=2");
    const f = schemaFields(adapters[0]?.settingsSchema);
    expect(f.map((x) => [x.key, x.required, x.defaultValue])).toEqual([
      ["port", true, null],
      ["outputDir", false, "dist"],
    ]);
    expect(targetKey({ Plugin: "acme/x" })).toBe("Plugin:acme/x");
    expect(statusTone("Failed")).toBe("danger");
    expect(statusTone("AwaitingApproval")).toBe("warning");
  });
});

describe("Deployments panel", () => {
  it("asks for a project", () => {
    render(<DeploymentsPanel projectId={null} />);
    expect(screen.getByText("No project open")).toBeTruthy();
  });

  it("shows environments, live version, history and adapter availability", async () => {
    render(<DeploymentsPanel projectId="p1" />);
    const detail = await screen.findByTestId("env-detail");
    await waitFor(() => expect(within(detail).getByTestId("env-current").textContent).toContain("v1"));
    expect(screen.getByTestId("env-production").textContent).toContain("Protected");
    await waitFor(() => expect(screen.getByTestId("deploy-log").textContent).toContain("copied 3 files"));
    fireEvent.click(screen.getByRole("tab", { name: "Adapters" }));
    expect(screen.getByTestId("adapter-availability-vercel").textContent).toBe("Needs vault secret VERCEL_TOKEN");
    expect(screen.getByTestId("adapter-vercel").textContent).toContain("Request shape verified");
  });

  it("creates an environment with a target, settings and a vault credential", async () => {
    render(<DeploymentsPanel projectId="p1" />);
    await screen.findByTestId("env-detail");
    fireEvent.click(screen.getByTestId("env-new"));
    const form = screen.getByTestId("env-form");
    fireEvent.change(within(form).getByTestId("env-name"), { target: { value: "staging" } });
    fireEvent.change(within(form).getByTestId("env-target"), { target: { value: "StaticSite" } });
    const save = within(form).getByTestId("env-save") as HTMLButtonElement;
    expect(save.disabled).toBe(true); // required `port` missing
    fireEvent.change(within(form).getByTestId("env-setting-port"), { target: { value: "8200" } });
    fireEvent.click(within(form).getByTestId("env-secret-APP_SECRET"));
    fireEvent.change(within(form).getByTestId("env-vars"), { target: { value: "MODE=staging" } });
    fireEvent.click(save);
    await waitFor(() => expect(lastCall("environments.create")).toBeTruthy());
    expect(lastCall("environments.create")).toMatchObject({
      projectId: "p1",
      name: "staging",
      kind: "Staging",
      target: "StaticSite",
      adapterSettings: { port: "8200" },
      secretRefs: ["sec-1"],
      variables: { MODE: "staging" },
    });
  });

  it("previews the plan, deploys, shows the approval state and approves", async () => {
    render(<DeploymentsPanel projectId="p1" />);
    await screen.findByTestId("env-detail");
    fireEvent.click(screen.getByTestId("deploy-open"));
    fireEvent.change(screen.getByTestId("deploy-version"), { target: { value: "v2" } });
    expect((screen.getByTestId("deploy-start") as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(screen.getByTestId("deploy-plan"));
    const plan = await screen.findByTestId("deploy-plan-view");
    expect(within(plan).getByTestId("plan-approval").textContent).toBe("Needs approval");
    expect(plan.textContent).toContain("Bearer [REDACTED]");
    fireEvent.click(screen.getByTestId("deploy-start"));
    await waitFor(() => expect(lastCall("deploy.start")).toEqual({ environmentId: "env-prod", version: "v2" }));
    const active = await screen.findByTestId("deploy-active");
    expect(within(active).getByTestId("active-status").textContent).toBe("Awaiting approval");
    fireEvent.click(await within(active).findByTestId("approve"));
    await waitFor(() => expect(lastCall("approvals.resolve")).toEqual({ approvalId: "ap-1", decision: "Approve" }));
  });

  it("rolls back and deletes only after confirmation", async () => {
    render(<DeploymentsPanel projectId="p1" />);
    await screen.findByTestId("env-detail");
    await waitFor(() => expect((screen.getByTestId("rollback") as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(screen.getByTestId("rollback"));
    expect(lastCall("deploy.rollback")).toBeUndefined();
    fireEvent.click(screen.getByTestId("rollback-confirm"));
    await waitFor(() => expect(lastCall("deploy.rollback")).toEqual({ environmentId: "env-prod" }));
    fireEvent.click(screen.getByTestId("env-delete"));
    expect(lastCall("environments.delete")).toBeUndefined();
    fireEvent.click(screen.getByTestId("env-delete-confirm"));
    await waitFor(() => expect(lastCall("environments.delete")).toEqual({ environmentId: "env-prod" }));
  });

  it("is translated to Russian", async () => {
    await i18n.changeLanguage("ru");
    render(<DeploymentsPanel projectId="p1" />);
    await screen.findByTestId("env-detail");
    expect(screen.getByTestId("env-production").textContent).toContain("Защищено");
    expect(screen.getByText("Откатить")).toBeTruthy();
  });
});
