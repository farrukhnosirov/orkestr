// useRpcQuery must not return the previous key's data after the key changed
// (audit r1: Replay requested the previous project's run after a switch).
import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const pending = new Map<string, { resolve: (v: unknown) => void; reject: (e: unknown) => void }>();
const fakeClient = {};
vi.mock("../core/client", () => ({ maybeClient: () => fakeClient }));

import { useAppStore } from "../store/app";
import { useRpcQuery } from "./rpcQuery";

function loaderFor(key: string) {
  return () =>
    new Promise((resolve, reject) => {
      pending.set(key, { resolve, reject });
    });
}

afterEach(() => {
  cleanup();
  pending.clear();
});

describe("useRpcQuery key changes", () => {
  it("drops the previous key's data while the new key loads and when it fails", async () => {
    useAppStore.setState((s) => ({ connection: { ...s.connection, state: "open" } }));
    const { result, rerender } = renderHook(({ k }: { k: string }) => useRpcQuery(k, loaderFor(k)), {
      initialProps: { k: "runs|A" },
    });
    await act(async () => pending.get("runs|A")?.resolve(["runA"]));
    expect(result.current.data).toEqual(["runA"]);

    rerender({ k: "runs|B" });
    expect(result.current.data).toBeNull();

    await act(async () => pending.get("runs|B")?.reject(new Error("boom")));
    expect(result.current.data).toBeNull();
    expect(result.current.error).not.toBeNull();

    rerender({ k: "runs|A" });
    expect(result.current.data).toBeNull();
    expect(result.current.error).toBeNull();
    await act(async () => pending.get("runs|A")?.resolve(["runA2"]));
    expect(result.current.data).toEqual(["runA2"]);
  });

  it("keeps data across same-key reloads", async () => {
    useAppStore.setState((s) => ({ connection: { ...s.connection, state: "open" } }));
    const { result } = renderHook(() => useRpcQuery("k", loaderFor("k")));
    await act(async () => pending.get("k")?.resolve(1));
    act(() => result.current.reload());
    expect(result.current.data).toBe(1);
    await act(async () => pending.get("k")?.reject(new Error("x")));
    expect(result.current.data).toBe(1);
  });
});
