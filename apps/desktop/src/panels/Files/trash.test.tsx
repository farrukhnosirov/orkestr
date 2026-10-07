/**
 * Regression: the delete dialog promises restore from Orkestr's trash, so the
 * app must list trashed copies (`files.trash`) and restore them
 * (`files.restore`), asking before it overwrites an existing file.
 */
import { OrkestrError } from "@orkestr/client";
import type { TrashItem } from "@orkestr/protocol";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { commands } from "../../features/commands/registry";
import { applyLanguagePreference } from "../../i18n";
import { TrashDialog } from "./TrashDialog";
import { useTrashUi } from "./trashStore";

const PID = "0190f5c2-0000-7000-8000-000000000077";
const calls: { method: string; params: Record<string, unknown> }[] = [];
let items: TrashItem[] = [];
let existing = new Set<string>();

const fake = {
  state: "open",
  call: vi.fn(async (method: string, params: Record<string, unknown>) => {
    calls.push({ method, params });
    if (method === "files.trash") return { items };
    if (method === "files.restore") {
      const item = items.find((i) => i.id === params.trashId);
      if (!item) throw new OrkestrError("not_found", "no entry");
      if (existing.has(item.path) && !params.overwrite) throw new OrkestrError("conflict", "exists", { reason: "exists" });
      items = items.map((i) => (i.id === item.id ? { ...i, restored: true } : i));
      return { path: item.path, hash: "h" };
    }
    return {};
  }),
  subscribe: vi.fn(() => () => undefined),
};

vi.mock("../../core/client", () => ({ getClient: () => fake, maybeClient: () => fake }));
vi.mock("../../components/workspaceActions", () => ({
  workspaceActions: { openPanel: vi.fn(), resetLayout: vi.fn() },
}));

function item(id: string, path: string): TrashItem {
  return { id, path, size: 2048, deletedAt: new Date().toISOString(), actor: "user", restored: false };
}

beforeEach(() => {
  calls.length = 0;
  items = [item("t1", "src/a.ts"), item("t2", "src/b.ts")];
  existing = new Set(["src/b.ts"]);
});
afterEach(() => cleanup());

describe("Orkestr trash", () => {
  it("lists trashed files and restores one", async () => {
    const onRestored = vi.fn();
    render(<TrashDialog projectId={PID} onClose={() => undefined} onRestored={onRestored} />);
    expect(await screen.findByText("src/a.ts")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Restore src/a.ts" }));
    await waitFor(() => expect(onRestored).toHaveBeenCalledWith("src/a.ts"));
    expect(calls.find((c) => c.method === "files.restore")?.params).toMatchObject({ projectId: PID, trashId: "t1", overwrite: false });
    expect(await screen.findByText("Restored src/a.ts.")).toBeTruthy();
  });

  it("asks before replacing an existing file, then overwrites", async () => {
    const onRestored = vi.fn();
    render(<TrashDialog projectId={PID} onClose={() => undefined} onRestored={onRestored} />);
    fireEvent.click(await screen.findByRole("button", { name: "Restore src/b.ts" }));
    expect(await screen.findByText("Replace src/b.ts?")).toBeTruthy();
    expect(onRestored).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Replace file" }));
    await waitFor(() => expect(onRestored).toHaveBeenCalledWith("src/b.ts"));
    const restores = calls.filter((c) => c.method === "files.restore").map((c) => c.params.overwrite);
    expect(restores).toEqual([false, true]);
  });

  it("shows an empty state", async () => {
    items = [];
    render(<TrashDialog projectId={PID} onClose={() => undefined} onRestored={() => undefined} />);
    expect(await screen.findByText("The trash is empty.")).toBeTruthy();
  });

  it("is reachable from the command palette", async () => {
    useTrashUi.setState({ open: false });
    await commands.find((c) => c.id === "files.openTrash")?.run({ projectId: PID });
    expect(useTrashUi.getState().open).toBe(true);
  });

  it("speaks Russian", async () => {
    await act(async () => {
      await applyLanguagePreference("ru");
    });
    try {
      render(<TrashDialog projectId={PID} onClose={() => undefined} onRestored={() => undefined} />);
      expect(await screen.findByText("Корзина Orkestr")).toBeTruthy();
      expect(screen.getAllByText("Восстановить").length).toBe(2);
      expect(commands.find((c) => c.id === "files.openTrash")?.title).toBe("Открыть корзину Orkestr…");
    } finally {
      await act(async () => {
        await applyLanguagePreference("en");
      });
    }
  });
});
