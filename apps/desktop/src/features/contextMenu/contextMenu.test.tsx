import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import "../../i18n";
import i18n from "i18next";
import type { AgentInstance, Task } from "@orkestr/protocol";

import { agentRowMenu } from "../../panels/Agents/AgentList";
import { taskRowMenu } from "../../panels/Tasks/ListView";
import { ContextMenuHost, contributeToEvent, installContextMenuHandler, useContextMenu } from "./ContextMenuProvider";
import { ContextMenuView } from "./ContextMenuView";
import { snippet } from "./defaults";
import { insertText } from "./editing";
import {
  edgeFocusable,
  mergeSections,
  type MenuEntry,
  nextFocusable,
  normalizeEntries,
  placeMenu,
  placeSubmenu,
  typeaheadMatch,
} from "./model";
import { closeContextMenu, setContextMenuEnvironment, useContextMenuStore } from "./store";

const act0 = (id: string, extra: Partial<MenuEntry> = {}): MenuEntry => ({ id, label: id, onSelect: () => {}, ...extra }) as MenuEntry;
const sep: MenuEntry = { kind: "separator" };

afterEach(() => {
  cleanup();
  closeContextMenu();
});

describe("model", () => {
  it("normalizes separators, hidden items and empty submenus", () => {
    const out = normalizeEntries([
      sep,
      act0("a"),
      sep,
      sep,
      act0("b", { hidden: true }),
      { kind: "submenu", id: "sub", label: "sub", items: [act0("x", { hidden: true })] },
      { kind: "label", id: "l", label: "L" },
      sep,
      act0("c"),
      sep,
    ]);
    expect(out.map((e) => ("id" in e ? e.id : "-"))).toEqual(["a", "-", "c"]);
  });

  it("merges sections with one separator between non-empty ones", () => {
    const out = mergeSections([[act0("a")], [], [act0("b")], [sep]]);
    expect(out.map((e) => e.kind ?? "action")).toEqual(["action", "separator", "action"]);
  });

  it("moves focus over focusable entries only, wrapping", () => {
    const list = [act0("a"), sep, act0("b", { disabled: true }), act0("c")];
    expect(edgeFocusable(list, "first")).toBe(0);
    expect(edgeFocusable(list, "last")).toBe(3);
    expect(nextFocusable(list, 0, 1)).toBe(3);
    expect(nextFocusable(list, 3, 1)).toBe(0);
    expect(nextFocusable(list, 0, -1)).toBe(3);
    expect(nextFocusable([sep], -1, 1)).toBe(-1);
  });

  it("type-ahead matches labels in any script and cycles on repeated letters", () => {
    const list = [act0("Копировать"), act0("Вставить"), act0("Вырезать"), act0("Copy")];
    expect(typeaheadMatch(list, -1, "в")).toBe(1);
    expect(typeaheadMatch(list, 1, "в")).toBe(2);
    expect(typeaheadMatch(list, 2, "вв")).toBe(1);
    expect(typeaheadMatch(list, 1, "вы")).toBe(2);
    expect(typeaheadMatch(list, 0, "co")).toBe(3);
    expect(typeaheadMatch(list, 0, "zz")).toBe(-1);
  });

  it("places menus inside the viewport, flipping at the edges", () => {
    const vp = { width: 800, height: 600 };
    expect(placeMenu({ x: 100, y: 100 }, { width: 200, height: 300 }, vp)).toEqual({ left: 100, top: 100 });
    expect(placeMenu({ x: 750, y: 550 }, { width: 200, height: 300 }, vp)).toEqual({ left: 550, top: 250 });
    // Too tall for either side: clamped so the bottom stays in the viewport.
    expect(placeMenu({ x: 10, y: 300 }, { width: 200, height: 590 }, vp).top).toBe(6);
    const sub = placeSubmenu({ left: 600, right: 790, top: 100, bottom: 128 }, { width: 200, height: 100 }, vp);
    expect(sub.side).toBe("left");
    expect(sub.left).toBe(402);
    expect(placeSubmenu({ left: 10, right: 210, top: 580, bottom: 600 }, { width: 200, height: 100 }, vp)).toMatchObject({ side: "right", top: 496 });
  });

  it("shortens long selections for labels", () => {
    expect(snippet("  hello \n world ")).toBe("hello world");
    expect(snippet("x".repeat(40), 10)).toBe(`${"x".repeat(9)}…`);
  });
});

describe("ContextMenuView", () => {
  it("navigates with the keyboard, opens submenus and activates items", async () => {
    const picked: string[] = [];
    const onClose = vi.fn();
    const items: MenuEntry[] = [
      { id: "a", label: "Alpha", onSelect: () => void picked.push("a") },
      { id: "b", label: "Beta", disabled: true, onSelect: () => void picked.push("b") },
      { kind: "checkbox", id: "c", label: "Check", checked: false, onSelect: (v) => void picked.push(`c:${v}`) },
      { kind: "submenu", id: "s", label: "More", items: [{ id: "s1", label: "Inner", onSelect: () => void picked.push("s1") }] },
    ];
    render(<ContextMenuView x={5} y={5} items={items} label="Menu" onClose={onClose} />);
    const root = screen.getByRole("menu", { name: "Menu" });
    expect(root.getAttribute("aria-activedescendant")).toBe(screen.getByText("Alpha").parentElement?.id);
    fireEvent.keyDown(root, { key: "ArrowDown" }); // skips disabled Beta
    expect(root.getAttribute("aria-activedescendant")).toBe(screen.getByRole("menuitemcheckbox").id);
    fireEvent.keyDown(root, { key: "ArrowDown" });
    fireEvent.keyDown(root, { key: "ArrowRight" });
    expect(screen.getAllByRole("menu")).toHaveLength(2);
    expect(screen.getByRole("menuitem", { name: "More" }).getAttribute("aria-expanded")).toBe("true");
    fireEvent.keyDown(root, { key: "ArrowLeft" });
    expect(screen.getAllByRole("menu")).toHaveLength(1);
    fireEvent.keyDown(root, { key: "c" }); // type-ahead → Check
    fireEvent.keyDown(root, { key: "Enter" });
    await act(async () => {
      await Promise.resolve();
    });
    expect(picked).toEqual(["c:true"]);
    expect(onClose).toHaveBeenCalledWith(true);
  });

  it("closes on Escape and on a click outside", () => {
    const onClose = vi.fn();
    render(<ContextMenuView x={5} y={5} items={[act0("a")]} label="Menu" onClose={onClose} />);
    fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" });
    expect(onClose).toHaveBeenCalledWith(true);
    const onClose2 = vi.fn();
    cleanup();
    render(<ContextMenuView x={5} y={5} items={[act0("a")]} label="Menu" onClose={onClose2} />);
    fireEvent.pointerDown(document.body);
    expect(onClose2).toHaveBeenCalledWith(false);
  });

  it("marks checkbox / radio state and danger items", () => {
    render(
      <ContextMenuView
        x={0}
        y={0}
        label="Menu"
        onClose={() => {}}
        items={[
          { kind: "radio", id: "r1", label: "One", checked: true, onSelect: () => {} },
          { kind: "radio", id: "r2", label: "Two", checked: false, onSelect: () => {} },
          act0("del", { danger: true } as Partial<MenuEntry>),
        ]}
      />,
    );
    const radios = screen.getAllByRole("menuitemradio");
    expect(radios.map((r) => r.getAttribute("aria-checked"))).toEqual(["true", "false"]);
    expect(screen.getByRole("menuitem").className).toContain("ok-ctxmenu__item--danger");
  });
});

function Row({ onPick }: { onPick: () => void }) {
  const menu = useContextMenu(() => [{ id: "row.open", label: "Open row", onSelect: onPick }]);
  return (
    <div data-testid="row" {...menu}>
      row text
    </div>
  );
}

describe("global handler", () => {
  let dispose: () => void;
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    dispose = installContextMenuHandler(() => "Context menu");
  });
  afterEach(() => dispose());

  const rightClick = (el: Element) => {
    const ev = new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 20, clientY: 30 });
    el.dispatchEvent(ev);
    return ev;
  };
  const labels = () => [...document.querySelectorAll(".ok-ctxmenu__label")].map((n) => n.textContent);

  it("replaces the browser menu with edit items on a text field", () => {
    render(
      <>
        <input aria-label="field" defaultValue="hello world" />
        <ContextMenuHost />
      </>,
    );
    const input = screen.getByLabelText("field") as HTMLInputElement;
    input.focus();
    input.setSelectionRange(0, 5);
    let ev!: MouseEvent;
    act(() => {
      ev = rightClick(input);
    });
    expect(ev.defaultPrevented).toBe(true);
    expect(labels()).toEqual(["Undo", "Redo", "Cut", "Copy", "Paste", "Select all"]);
    // Focus stays in the field (inline editors must not blur).
    expect(document.activeElement).toBe(input);
    expect(useContextMenuStore.getState().menu?.keepFocus).toBe(true);
  });

  it("adds selection items with the window environment", () => {
    const restore = setContextMenuEnvironment({ searchInOrkestr: () => {}, askManager: () => {} });
    render(
      <>
        <p data-testid="p">some selected words</p>
        <ContextMenuHost />
      </>,
    );
    const p = screen.getByTestId("p");
    const range = document.createRange();
    range.selectNodeContents(p);
    window.getSelection()?.removeAllRanges();
    window.getSelection()?.addRange(range);
    act(() => void rightClick(p));
    expect(labels()).toEqual(["Copy", "Search Orkestr for “some selected words”", "Ask Manager about selection"]);
    window.getSelection()?.removeAllRanges();
    restore();
  });

  it("puts contributed items first, then link items, in Russian too", async () => {
    await i18n.changeLanguage("ru");
    const pick = vi.fn();
    render(
      <>
        <a href="https://example.com/x">
          <Row onPick={pick} />
        </a>
        <ContextMenuHost />
      </>,
    );
    act(() => void rightClick(screen.getByTestId("row")));
    expect(labels()).toEqual(["Open row", "Открыть ссылку", "Копировать адрес ссылки"]);
    fireEvent.click(screen.getByText("Open row"));
    await act(async () => {
      await Promise.resolve();
    });
    expect(pick).toHaveBeenCalled();
  });

  it("leaves events a component already handled (Monaco) alone, and shows nothing for empty areas", () => {
    render(
      <>
        <div data-testid="own" onContextMenu={(e) => e.preventDefault()} />
        <div data-testid="empty" />
        <ContextMenuHost />
      </>,
    );
    act(() => void rightClick(screen.getByTestId("own")));
    expect(useContextMenuStore.getState().menu).toBeNull();
    let ev!: MouseEvent;
    act(() => {
      ev = rightClick(screen.getByTestId("empty"));
    });
    expect(ev.defaultPrevented).toBe(true); // never the browser menu
    expect(useContextMenuStore.getState().menu).toBeNull();
  });

  it("replaceDefaults drops the default items", () => {
    render(<div data-testid="t" />);
    const el = screen.getByTestId("t");
    const ev = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    contributeToEvent(ev, [act0("only")], { replaceDefaults: true });
    el.dispatchEvent(ev);
    expect(useContextMenuStore.getState().menu?.items.map((i) => ("id" in i ? i.id : ""))).toEqual(["only"]);
  });
});

describe("editing", () => {
  it("inserts text into controlled fields when execCommand is unavailable", () => {
    const onInput = vi.fn();
    render(<input aria-label="f" defaultValue="abcdef" onInput={onInput} />);
    const input = screen.getByLabelText("f") as HTMLInputElement;
    input.setSelectionRange(1, 3);
    insertText(input, "XY");
    expect(input.value).toBe("aXYdef");
    expect(onInput).toHaveBeenCalled();
  });
});

describe("per-context item sets", () => {
  const tt = (k: string) => k;

  it("agent rows: open, pause/stop only while active, copy", () => {
    const base = { id: "ag1", name: "Builder", state: "Running", activity: null, worktree: { branch: "orkestr/ag1", path: "/w" } } as unknown as AgentInstance;
    const visible = (a: AgentInstance) => normalizeEntries(agentRowMenu(a, tt)).flatMap((e) => (e.kind !== "separator" && "id" in e && e.id ? [e.id] : []));
    expect(visible(base)).toEqual(["agent.open", "agent.pause", "agent.stop", "agent.copyName", "agent.copyId", "agent.copyBranch"]);
    expect(visible({ ...base, state: "Completed", worktree: null } as unknown as AgentInstance)).toEqual(["agent.open", "agent.copyName", "agent.copyId"]);
  });

  it("task rows: open, ask Manager, copy", () => {
    const task = { id: "t1", title: "Fix it", projectId: "p1" } as unknown as Task;
    expect(taskRowMenu(task, tt, () => {}).flatMap((e) => (e.kind !== "separator" && "id" in e && e.id ? [e.id] : []))).toEqual(["task.open", "task.ask", "task.copyTitle", "task.copyId"]);
  });
});
