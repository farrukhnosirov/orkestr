// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { Button, EmptyState, IconButton, Input, setUiStrings, Spinner, StatusDot, Tabs } from "./components";

afterEach(cleanup);

describe("primitives", () => {
  it("default strings are localizable (setUiStrings)", () => {
    render(<Spinner />);
    expect(screen.getByRole("status").getAttribute("aria-label")).toBe("Loading");
    cleanup();
    setUiStrings({ loading: "Загрузка…" });
    render(<Spinner />);
    expect(screen.getByRole("status").getAttribute("aria-label")).toBe("Загрузка…");
    setUiStrings({ loading: "Loading" });
  });

  it("Button handles clicks and loading state", () => {
    const onClick = vi.fn();
    const { rerender } = render(<Button onClick={onClick}>Create</Button>);
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    expect(onClick).toHaveBeenCalledTimes(1);
    rerender(
      <Button onClick={onClick} loading>
        Create
      </Button>,
    );
    const btn = screen.getByRole("button");
    expect(btn).toHaveProperty("disabled", true);
    expect(btn.getAttribute("aria-busy")).toBe("true");
  });

  it("IconButton exposes its label", () => {
    render(<IconButton label="Close window" icon={<span>x</span>} />);
    expect(screen.getByRole("button", { name: "Close window" })).toBeTruthy();
  });

  it("Input wires label and error", () => {
    render(<Input label="Project name" error="Required" />);
    const input = screen.getByLabelText("Project name");
    expect(input.getAttribute("aria-invalid")).toBe("true");
    expect(screen.getByRole("alert").textContent).toBe("Required");
  });

  it("Tabs supports keyboard navigation", () => {
    function Harness() {
      const [v, setV] = useState<"a" | "b" | "c">("a");
      return (
        <Tabs
          aria-label="demo"
          value={v}
          onChange={setV}
          items={[
            { id: "a", label: "A" },
            { id: "b", label: "B", disabled: true },
            { id: "c", label: "C" },
          ]}
        />
      );
    }
    render(<Harness />);
    const list = screen.getByRole("tablist");
    fireEvent.keyDown(list, { key: "ArrowRight" });
    expect(screen.getByRole("tab", { name: "C" }).getAttribute("aria-selected")).toBe("true");
    fireEvent.keyDown(list, { key: "ArrowRight" });
    expect(screen.getByRole("tab", { name: "A" }).getAttribute("aria-selected")).toBe("true");
  });

  it("EmptyState and StatusDot render", () => {
    render(
      <>
        <EmptyState title="Nothing yet" description="Arrives in Phase 1" />
        <StatusDot status="online" label="Connected" />
      </>,
    );
    expect(screen.getByText("Nothing yet")).toBeTruthy();
    expect(screen.getByRole("img", { name: "Connected" })).toBeTruthy();
  });
});
