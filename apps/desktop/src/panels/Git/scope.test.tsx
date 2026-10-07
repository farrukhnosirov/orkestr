/**
 * Regression: switching projects must not leave project A's Git data or
 * selected snapshot on screen (and actionable) under project B.
 */
import type { WorkspaceSnapshot } from "@orkestr/protocol";
import { act, cleanup, fireEvent, render, renderHook, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const A = "0190f0b0-0000-7000-8000-00000000000a";
const B = "0190f0b0-0000-7000-8000-00000000000b";

const lists: Record<string, WorkspaceSnapshot[] | Error> = {};
const calls: { method: string; params: Record<string, unknown> }[] = [];

vi.mock("../../core/client", () => ({
  maybeClient: () => null,
  getClient: () => ({
    call: (method: string, params: Record<string, unknown>) => {
      calls.push({ method, params });
      if (method === "snapshots.list") {
        const r = lists[params.projectId as string];
        return r instanceof Error ? Promise.reject(r) : Promise.resolve({ snapshots: r ?? [] });
      }
      if (method === "snapshots.diff") return Promise.resolve({ changes: [] });
      return Promise.reject(new Error(`unexpected ${method}`));
    },
  }),
}));

const { SnapshotsTab } = await import("./SnapshotsTab");
const { useAsync, gitScope } = await import("./state");

function snap(projectId: string, id: string, label: string): WorkspaceSnapshot {
  return {
    id,
    projectId,
    workspaceId: "w",
    taskId: null,
    label,
    reason: "Manual",
    baseCommit: null,
    manifestObjectId: "m",
    includesUncommitted: true,
    fileCount: 1,
    sizeBytes: 10,
    createdBy: { kind: "user" },
    createdAt: "2026-10-07T00:00:00Z",
  } as unknown as WorkspaceSnapshot;
}

beforeEach(() => {
  calls.length = 0;
  for (const k of Object.keys(lists)) delete lists[k];
});
afterEach(() => cleanup());

describe("useAsync scope", () => {
  it("drops the previous scope's data when the new scope fails to load", async () => {
    const results: Record<string, Promise<string>> = { a: Promise.resolve("A-data"), b: Promise.reject(new Error("boom")) };
    results.b!.catch(() => undefined);
    const { result, rerender } = renderHook(({ s }: { s: string }) => useAsync(() => results[s]!, [s], s), {
      initialProps: { s: "a" },
    });
    await waitFor(() => expect(result.current.data).toBe("A-data"));
    rerender({ s: "b" });
    // Never A's data under B, not even while B is loading.
    expect(result.current.data).toBeNull();
    await waitFor(() => expect(result.current.error).toBeTruthy());
    expect(result.current.data).toBeNull();
  });

  it("keeps data across reloads within the same scope", async () => {
    let n = 0;
    const { result } = renderHook(() => useAsync(() => Promise.resolve(++n), [], "same"));
    await waitFor(() => expect(result.current.data).toBe(1));
    act(() => result.current.reload());
    expect(result.current.data).toBe(1);
    await waitFor(() => expect(result.current.data).toBe(2));
  });

  it("scopes by project and worktree", () => {
    expect(gitScope(A)).not.toBe(gitScope(B));
    expect(gitScope(A, "C:/wt")).not.toBe(gitScope(A));
  });
});

describe("SnapshotsTab across a project switch", () => {
  it("does not keep project A's selected snapshot under project B", async () => {
    lists[A] = [snap(A, "sa", "Snapshot of A")];
    lists[B] = new Error("not a repo");
    const { rerender } = render(<SnapshotsTab projectId={A} worktree={null} refresh={0} />);
    const item = await screen.findByText("Snapshot of A");
    fireEvent.click(item);
    await waitFor(() => expect(screen.getAllByText(/Snapshot of A/).length).toBeGreaterThan(1));

    // Same component instance, new project (as the dock does without a key).
    rerender(<SnapshotsTab projectId={B} worktree={null} refresh={0} />);
    await waitFor(() => expect(calls.some((c) => c.method === "snapshots.list" && c.params.projectId === B)).toBe(true));
    await waitFor(() => expect(screen.queryByText(/Snapshot of A/)).toBeNull());
  });
});
