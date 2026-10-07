/**
 * Rollback Task (§25, §63.J) is reachable from the UI: dry run → plan with
 * conflicts → explicit confirmation over `snapshots.rollbackTask`.
 */
import { OrkestrError } from "@orkestr/client";
import type { RollbackResult } from "@orkestr/protocol";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { commands } from "../commands/registry";
import { applyLanguagePreference } from "../../i18n";
import { useTasksUi } from "../../panels/Tasks/store";
import { TaskRollbackButton, TaskRollbackDialog, canRollbackTask, taskRollbackSummary } from "./index";

const PID = "0190f5c2-0000-7000-8000-0000000000a1";
const TID = "0190f5c2-0000-7000-8000-0000000000b2";
const calls: { method: string; params: Record<string, unknown> }[] = [];
let plan: RollbackResult | Error;

function result(patch: Partial<RollbackResult> = {}): RollbackResult {
  return {
    snapshotId: "s1",
    preRollbackSnapshotId: null,
    restored: ["src/a.ts"],
    deleted: ["src/new.ts"],
    conflicts: [{ path: "src/shared.ts", reason: "changed by someone else after the task — not overwritten" }],
    unchanged: [],
    dryRun: true,
    ...patch,
  };
}

const fake = {
  state: "open",
  call: vi.fn(async (method: string, params: Record<string, unknown>) => {
    calls.push({ method, params });
    if (method === "snapshots.rollbackTask") {
      if (params.dryRun) {
        if (plan instanceof Error) throw plan;
        return plan;
      }
      return result({ dryRun: false, preRollbackSnapshotId: "pre" });
    }
    return {};
  }),
  subscribe: vi.fn(() => () => undefined),
};

vi.mock("../../core/client", () => ({ getClient: () => fake, maybeClient: () => fake }));
vi.mock("../../components/workspaceActions", () => ({
  workspaceActions: { openPanel: vi.fn(), resetLayout: vi.fn() },
}));

beforeEach(() => {
  calls.length = 0;
  plan = result();
});
afterEach(() => cleanup());

describe("Rollback Task dialog", () => {
  it("shows the dry-run plan, then rolls back only after confirmation", async () => {
    const onDone = vi.fn();
    render(<TaskRollbackDialog projectId={PID} taskId={TID} taskTitle="Add login" onClose={() => undefined} onDone={onDone} />);
    expect(await screen.findByText("src/a.ts")).toBeTruthy();
    expect(screen.getByText("src/new.ts")).toBeTruthy();
    expect(screen.getByText("src/shared.ts")).toBeTruthy();
    expect(screen.getByText("changed by someone else after the task — not overwritten")).toBeTruthy();
    // The dry run never applies anything.
    expect(calls).toEqual([{ method: "snapshots.rollbackTask", params: { projectId: PID, taskId: TID, dryRun: true, confirm: false } }]);

    fireEvent.click(screen.getByRole("button", { name: "Roll back 2 files" }));
    await waitFor(() => expect(onDone).toHaveBeenCalled());
    expect(calls[1]).toEqual({ method: "snapshots.rollbackTask", params: { projectId: PID, taskId: TID, dryRun: false, confirm: true } });
    expect(onDone.mock.calls[0]?.[0]).toContain("Rolled back task “Add login”: 1 restored, 1 deleted, 1 left as is");
  });

  it("cannot confirm when nothing can be rolled back safely", async () => {
    plan = result({ restored: [], deleted: [] });
    render(<TaskRollbackDialog projectId={PID} taskId={TID} taskTitle="T" onClose={() => undefined} onDone={() => undefined} />);
    expect(await screen.findByText(/Nothing can be rolled back safely/)).toBeTruthy();
    expect((screen.getByRole("button", { name: "Roll back 0 files" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("explains a task without checkpoints", async () => {
    plan = new OrkestrError("not_found", "task has no before/after checkpoint pair");
    render(<TaskRollbackDialog projectId={PID} taskId={TID} taskTitle="T" onClose={() => undefined} onDone={() => undefined} />);
    expect(await screen.findByText(/This task has no before\/after snapshots/)).toBeTruthy();
  });

  it("is offered from a button and only for tasks that may have changed files", async () => {
    expect(canRollbackTask("Completed")).toBe(true);
    expect(canRollbackTask("Failed")).toBe(true);
    expect(canRollbackTask("Planned")).toBe(false);
    expect(canRollbackTask("Running")).toBe(false);
    render(<TaskRollbackButton projectId={PID} taskId={TID} taskTitle="T" onDone={() => undefined} />);
    fireEvent.click(screen.getByRole("button", { name: "Roll back task…" }));
    expect(await screen.findByText("Roll back task “T”?")).toBeTruthy();
  });

  it("the palette command targets the selected task", async () => {
    const cmd = commands.find((c) => c.id === "tasks.rollbackTask");
    expect(cmd).toBeTruthy();
    useTasksUi.setState({ selectedTaskId: null, rollbackRequest: null });
    expect(cmd?.enabled?.({ projectId: PID })).toBe(false);
    useTasksUi.setState({ selectedTaskId: TID });
    expect(cmd?.enabled?.({ projectId: PID })).toBe(true);
    await cmd?.run({ projectId: PID });
    expect(useTasksUi.getState().rollbackRequest).toBe(TID);
  });

  it("speaks Russian", async () => {
    await act(async () => {
      await applyLanguagePreference("ru");
    });
    try {
      render(<TaskRollbackDialog projectId={PID} taskId={TID} taskTitle="Вход" onClose={() => undefined} onDone={() => undefined} />);
      expect(await screen.findByText("Откатить задачу «Вход»?")).toBeTruthy();
      expect(screen.getByText("после задачи изменён кем-то другим — не перезаписан")).toBeTruthy();
      expect(screen.getByRole("button", { name: "Откатить 2 файла" })).toBeTruthy();
      expect(taskRollbackSummary("Вход", result({ conflicts: [] }))).toContain("Задача «Вход» откачена");
      expect(commands.find((c) => c.id === "tasks.rollbackTask")?.title).toBe("Откатить выбранную задачу…");
    } finally {
      await act(async () => {
        await applyLanguagePreference("en");
      });
    }
  });
});
