import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { applyLanguagePreference } from "../../i18n";
import { LayoutDialogs } from "./LayoutDialogs";
import { LayoutEditorPanel } from "./LayoutEditor";
import { LayoutSwitcher } from "./LayoutSwitcher";
import { useLayoutStore } from "./store";

const DEVICE = "0190a000-0000-7000-8000-000000000001";

afterEach(async () => {
  cleanup();
  useLayoutStore.getState().patch({ editorOpen: false, dialog: null, state: null, activeId: "coding", locked: false, loaded: false });
  await applyLanguagePreference("en");
});

describe("layout UI", () => {
  it("switcher lists presets and user layouts", async () => {
    useLayoutStore.getState().patch({
      loaded: true,
      activeId: "u-deep",
      state: {
        deviceId: DEVICE,
        activeId: "u-deep",
        layouts: [
          {
            schemaVersion: 1,
            id: "u-deep",
            name: "Deep work",
            dock: {},
            panels: ["code"],
            locked: false,
            windows: { mainMaximized: false, popouts: [{ panelId: "logs", maximized: false }] },
          },
        ],
      },
    });
    render(<LayoutSwitcher />);
    const button = screen.getByRole("button", { name: "Layout: Deep work" });
    fireEvent.click(button);
    expect(screen.getByRole("menuitemradio", { name: /Mission Control/ })).toBeTruthy();
    expect(screen.getByRole("menuitemradio", { name: /Deep work/ }).getAttribute("aria-checked")).toBe("true");
    expect(screen.getByText("1 window")).toBeTruthy();
  });

  it("editor sheet renders in Russian with panels and the lock switch", async () => {
    await act(async () => {
      await applyLanguagePreference("ru");
    });
    act(() => useLayoutStore.getState().patch({ editorOpen: true }));
    render(<LayoutEditorPanel />);
    expect(screen.getByRole("dialog", { name: "Редактор раскладки" })).toBeTruthy();
    expect(screen.getByText("Раскладки")).toBeTruthy();
    expect(screen.getByText("Панели")).toBeTruthy();
    expect(screen.getByText("Мониторы")).toBeTruthy();
    expect(screen.getByText("Разработка")).toBeTruthy();
    // Outside the desktop shell, monitors are explained instead of shown.
    expect(screen.getByText("Отдельные окна и мониторы доступны в настольном приложении.")).toBeTruthy();
    expect(screen.getByRole("switch").getAttribute("aria-checked")).toBe("false");
  });

  it("save dialog validates the name", async () => {
    act(() => useLayoutStore.getState().patch({ dialog: { kind: "saveAs" } }));
    render(<LayoutDialogs />);
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByText("Enter a name of 1–80 characters.")).toBeTruthy();
  });
});
