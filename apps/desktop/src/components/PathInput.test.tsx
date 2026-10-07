// "Browse…" next to every path field: native dialog in the desktop shell,
// hidden in a browser (the path is typed).
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

let native = false;
const picks: { kind: string; opts: unknown }[] = [];
let picked: string | null = null;
vi.mock("../lib/tauri", () => ({
  canPickPaths: () => native,
  pickFolder: (opts: unknown) => {
    picks.push({ kind: "folder", opts });
    return Promise.resolve(picked);
  },
  pickFile: (opts: unknown) => {
    picks.push({ kind: "file", opts });
    return Promise.resolve(picked);
  },
}));

import { i18n } from "../i18n";
import { PathInput } from "./PathInput";

beforeEach(async () => {
  await i18n.changeLanguage("en");
  native = false;
  picks.length = 0;
  picked = null;
});
afterEach(async () => {
  cleanup();
  await i18n.changeLanguage("en");
});

describe("PathInput", () => {
  it("hides Browse outside the desktop shell and stays typeable", () => {
    const onChange = vi.fn();
    render(<PathInput label="Folder" value="" onPathChange={onChange} />);
    expect(screen.queryByRole("button", { name: /Browse/ })).toBeNull();
    fireEvent.change(screen.getByLabelText("Folder"), { target: { value: "C:\\x" } });
    expect(onChange).toHaveBeenCalledWith("C:\\x");
  });

  it("fills the field with the picked folder (mapped when asked)", async () => {
    native = true;
    picked = "C:\\Code\\app";
    const onChange = vi.fn();
    render(<PathInput label="Folder" value={"C:\\Code"} onPathChange={onChange} mapPicked={(p) => `${p}!`} dialogTitle="Pick" />);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /Browse…/ }));
      await Promise.resolve();
    });
    expect(picks).toEqual([{ kind: "folder", opts: { title: "Pick", defaultPath: "C:\\Code" } }]);
    expect(onChange).toHaveBeenCalledWith("C:\\Code\\app!");
  });

  it("offers a file picker too when a file or a folder is accepted, and ignores a cancel", async () => {
    native = true;
    const onChange = vi.fn();
    render(<PathInput label="Source" value="" kind="both" onPathChange={onChange} />);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Choose a file with the system dialog" }));
      await Promise.resolve();
    });
    expect(picks[0]?.kind).toBe("file");
    expect(onChange).not.toHaveBeenCalled();
  });

  it("is translated", async () => {
    native = true;
    await i18n.changeLanguage("ru");
    render(<PathInput label="Папка" value="" onPathChange={() => {}} />);
    expect(screen.getByRole("button", { name: /Обзор…/ })).toBeTruthy();
  });
});
