import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { applyLanguagePreference } from "../../i18n";
import type { PublisherKey, RegistrySource } from "./api";

const FP =
  "ab12 cd34 ef56 0011 2233 4455 6677 8899 aabb ccdd eeff 0123 4567 89ab cdef 0f0f";

const calls: Array<{ method: string; params: Record<string, unknown> }> = [];
let local: PublisherKey | undefined;
let publishError: string | null = null;

const sources: RegistrySource[] = [
  { id: "official", name: "Orkestr official", kind: "Official", location: "bundled", enabled: true, entries: 2 },
  { id: "dir-1", name: "Team folder", kind: "Directory", location: "C:\\reg", enabled: true, entries: 0 },
];

const fakeClient = {
  call: vi.fn(async (method: string, params: Record<string, unknown>) => {
    calls.push({ method, params });
    switch (method) {
      case "marketplace.sources":
        return { sources };
      case "marketplace.keys":
        return { keys: [], localPublisher: local };
      case "marketplace.publish":
        if (publishError) throw new Error(publishError);
        return {
          entry: {
            id: "dev.hello-widget",
            kind: params.kind,
            name: "Hello widget",
            version: "0.1.0",
            publisher: "dev",
            description: "",
            keyId: "k-local",
            sha256: "beef",
            url: "packages/x",
            revoked: false,
            sourceId: params.sourceId,
          },
        };
      default:
        throw new Error(`unexpected ${method}`);
    }
  }),
  subscribe: vi.fn(() => () => {}),
};

vi.mock("../../core/client", () => ({
  maybeClient: () => fakeClient,
  getClient: () => fakeClient,
}));

const { PublishTab, guessKind, publishableSources } = await import("./PublishTab");

beforeEach(() => {
  applyLanguagePreference("en");
  calls.length = 0;
  publishError = null;
  local = {
    keyId: "k-local",
    publisher: "dev",
    publicKey: "pk",
    fingerprint: FP,
    trust: "Trusted",
    firstSeen: "2026-10-01T00:00:00Z",
  };
});

afterEach(() => cleanup());

describe("publish helpers", () => {
  it("only offers writable sources and guesses kinds", () => {
    expect(publishableSources(sources).map((s) => s.id)).toEqual(["dir-1"]);
    expect(guessKind("a/agent.json")).toBe("AgentTemplate");
    expect(guessKind("hello-widget.orkp")).toBe("Widget");
    expect(guessKind("tool.orkp")).toBe("Plugin");
    expect(guessKind("x.zip")).toBeNull();
  });
});

describe("PublishTab", () => {
  it("shows the local key fingerprint, publishes to the chosen source and shows the result", async () => {
    const onChanged = vi.fn();
    render(<PublishTab version={0} onChanged={onChanged} />);
    expect(await screen.findByText(/Publishing as dev/)).toBeTruthy();
    expect(screen.getByText(FP.split(" ").slice(0, 8).join(" "))).toBeTruthy();
    fireEvent.change(screen.getByLabelText("Package file"), {
      target: { value: "C:\\out\\hello-widget.orkp" },
    });
    expect((screen.getByLabelText("Kind") as HTMLSelectElement).value).toBe("Widget");
    await waitFor(() =>
      expect((screen.getByLabelText("Target source") as HTMLSelectElement).value).toBe("dir-1"),
    );
    fireEvent.click(screen.getByRole("button", { name: "Sign and publish" }));
    await waitFor(() =>
      expect(calls.find((c) => c.method === "marketplace.publish")?.params).toEqual({
        path: "C:\\out\\hello-widget.orkp",
        kind: "Widget",
        sourceId: "dir-1",
        publisher: undefined,
      }),
    );
    expect(await screen.findByText("Published Hello widget 0.1.0")).toBeTruthy();
    expect(onChanged).toHaveBeenCalled();
  });

  it("asks for a publisher name before the first publish and shows errors", async () => {
    local = undefined;
    publishError = "directory registry is not signed by this publisher key";
    render(<PublishTab version={0} onChanged={() => {}} />);
    expect(await screen.findByText(/No publisher key yet/)).toBeTruthy();
    fireEvent.change(screen.getByLabelText("Package file"), {
      target: { value: "C:\\out\\agent.json" },
    });
    const submit = screen.getByRole("button", { name: "Sign and publish" }) as HTMLButtonElement;
    expect(submit.disabled).toBe(true);
    fireEvent.change(screen.getByLabelText(/Publisher name/), { target: { value: "acme" } });
    await waitFor(() => expect(submit.disabled).toBe(false));
    fireEvent.click(submit);
    expect(await screen.findByText(/Publish failed: .*not signed by this publisher key/)).toBeTruthy();
    expect(calls.find((c) => c.method === "marketplace.publish")!.params).toMatchObject({
      kind: "AgentTemplate",
      publisher: "acme",
    });
  });

  it("renders in Russian", async () => {
    applyLanguagePreference("ru");
    render(<PublishTab version={0} onChanged={() => {}} />);
    expect(await screen.findByRole("button", { name: "Подписать и опубликовать" })).toBeTruthy();
  });
});
