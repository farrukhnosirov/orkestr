/**
 * Regression: command `enabled()` must be evaluated when the palette opens.
 * Items were memoized once at mount, so a command that was disabled at
 * startup (e.g. "Open voice commands" before the voice config loaded) stayed
 * disabled and Enter silently did nothing (found in the Phase 3 acceptance run).
 */
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it } from "vitest";

import { type AppCommand, commands } from "../features/commands/registry";
import { i18n } from "../i18n";
import { useAppStore } from "../store/app";
import { registerLayoutFeature } from "../features/layout/register";
import { CommandPalette, uniqueById } from "./CommandPalette";

let ready = false;
let ran = 0;
const probe: AppCommand = {
  id: "test.paletteProbe",
  title: "Zebra probe command",
  group: "Test",
  enabled: () => ready,
  run: () => {
    ran += 1;
  },
};

beforeAll(async () => {
  await i18n.changeLanguage("en");
  commands.push(probe);
});

afterEach(() => {
  cleanup();
  act(() => useAppStore.getState().setPaletteOpen(false));
});

function openAndFind(): HTMLElement {
  act(() => useAppStore.getState().setPaletteOpen(true));
  fireEvent.change(screen.getByRole("combobox"), { target: { value: "Zebra probe" } });
  const el = screen.getAllByRole("option").find((o) => o.textContent?.includes("Zebra probe command"));
  if (!el) throw new Error(`probe not listed: ${screen.getAllByRole("option").map((o) => o.textContent).join(" | ").slice(0, 300)}`);
  return el;
}

describe("command palette enablement", () => {
  it("re-evaluates enabled() every time it opens", () => {
    render(<CommandPalette />);
    expect(openAndFind().getAttribute("aria-disabled")).toBe("true");
    act(() => useAppStore.getState().setPaletteOpen(false));
    ready = true;
    const item = openAndFind();
    expect(item.getAttribute("aria-disabled")).toBeNull();
    fireEvent.keyDown(item, { key: "Enter" });
    expect(ran).toBe(1);
    commands.splice(commands.indexOf(probe), 1);
  });
});

describe("palette item ids", () => {
  it("lists 'Reset layout' once and never renders duplicate option ids", () => {
    registerLayoutFeature();
    render(<CommandPalette />);
    act(() => useAppStore.getState().setPaletteOpen(true));
    fireEvent.change(screen.getByRole("combobox"), { target: { value: "reset layout" } });
    expect(screen.getAllByRole("option").filter((o) => o.id === "palette-layout.reset")).toHaveLength(1);
    fireEvent.change(screen.getByRole("combobox"), { target: { value: "" } });
    const ids = screen.getAllByRole("option").map((o) => o.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("uniqueById keeps the first item of each id", () => {
    expect(uniqueById([{ id: "a", n: 1 }, { id: "b", n: 2 }, { id: "a", n: 3 }])).toEqual([
      { id: "a", n: 1 },
      { id: "b", n: 2 },
    ]);
  });
});
