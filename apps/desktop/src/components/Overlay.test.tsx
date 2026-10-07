// Audit r2: panel dialogs were rendered inline inside their dock group. The
// group has `backdrop-filter`, which makes it the containing block of the
// `position: fixed` `.overlay`, so dialogs were sized to and clipped by the
// group instead of covering the window. Every overlay now goes through
// <Overlay>, which portals to <body>.
import { readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ContextMenu } from "../panels/Files/ContextMenu";
import { Overlay } from "./Overlay";

afterEach(cleanup);

const SRC = join(dirname(fileURLToPath(import.meta.url)), "..");

function walk(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    const full = join(dir, name);
    if (statSync(full).isDirectory()) walk(full, out);
    else if (full.endsWith(".tsx") && !full.endsWith(".test.tsx")) out.push(full);
  }
  return out;
}

describe("Overlay", () => {
  it("renders as a direct child of <body>, outside a filtered dock group", () => {
    render(
      <div className="dv-groupview" style={{ backdropFilter: "blur(4px)" }} data-testid="group">
        <Overlay className="overlay--top">
          <div role="dialog" aria-label="probe" />
        </Overlay>
      </div>,
    );
    const overlay = screen.getByRole("dialog", { name: "probe" }).parentElement!;
    expect(overlay.classList.contains("overlay")).toBe(true);
    expect(overlay.classList.contains("overlay--top")).toBe(true);
    expect(overlay.parentElement).toBe(document.body);
    expect(screen.getByTestId("group").contains(overlay)).toBe(false);
  });

  it("dismisses only on a backdrop mouse-down", () => {
    const onDismiss = vi.fn();
    render(
      <Overlay onDismiss={onDismiss}>
        <div role="dialog" aria-label="probe" />
      </Overlay>,
    );
    const dialog = screen.getByRole("dialog", { name: "probe" });
    fireEvent.mouseDown(dialog);
    expect(onDismiss).not.toHaveBeenCalled();
    fireEvent.mouseDown(dialog.parentElement!);
    expect(onDismiss).toHaveBeenCalledTimes(1);
  });

  it("the file context menu is portaled to <body> too", () => {
    render(
      <div data-testid="group">
        <ContextMenu x={10} y={10} label="menu" onClose={() => {}} items={[]} />
      </div>,
    );
    const menu = screen.getByRole("menu", { name: "menu" });
    expect(menu.parentElement).toBe(document.body);
  });

  it("no component renders an inline `.overlay` div (use <Overlay>)", () => {
    const offenders = walk(SRC)
      .filter((f) => !f.endsWith(join("components", "Overlay.tsx")))
      .filter((f) => /className=\{?["'`]overlay(?:\s|["'`])/.test(readFileSync(f, "utf8")))
      .map((f) => relative(SRC, f));
    expect(offenders).toEqual([]);
  });
});
