import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { LanguageSelect } from "../components/LanguageSelect";
import { StatusBar } from "../components/StatusBar";
import { coreCommands } from "../features/commands/coreCommands";
import { installLanguageEffects } from "../lib/language";
import { PANELS } from "../panels/registry";
import { applyLanguagePreference, i18n } from "./index";

afterEach(async () => {
  cleanup();
  await act(async () => {
    await applyLanguagePreference("en");
  });
});

describe("live language switching", () => {
  it("re-renders the shell, registries and <html lang> without a reload", async () => {
    installLanguageEffects();
    render(
      <>
        <LanguageSelect />
        <StatusBar />
      </>,
    );
    expect(screen.getByText("Commands")).toBeTruthy();
    const select = screen.getByRole("combobox", { name: "Language" });
    await act(async () => {
      fireEvent.change(select, { target: { value: "ru" } });
    });
    await screen.findByText("Команды");
    expect(i18n.language).toBe("ru");
    expect(document.documentElement.lang).toBe("ru");
    expect(screen.getByRole("combobox", { name: "Язык" })).toBeTruthy();
    // Registries are getters: they follow the switch too.
    expect(PANELS.find((p) => p.id === "logs")?.title).toBe("Журнал");
    const switchCmd = coreCommands.find((c) => c.id === "language.switch");
    expect(switchCmd?.title).toBe("Сменить язык / Switch Language");
    expect(coreCommands.find((c) => c.id === "project.create")?.title).toBe("Создать проект");
  });
});
