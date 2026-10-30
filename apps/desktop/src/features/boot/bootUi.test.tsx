import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { BootHost } from "./BootHost";
import { APP_VERSION } from "./plan";
import { LAST_VERSION_KEY, PREFS_KEY } from "./prefs";
import { useBootStore } from "./store";

describe("BootHost", () => {
  beforeEach(() => {
    window.localStorage.clear();
    useBootStore.setState({ finished: false, spokeGreeting: false, preview: null });
  });
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it("plays over the connection screen and leaves on a key press", async () => {
    render(<BootHost ready={false} compact={false} />);
    const overlay = screen.getByTestId("boot-overlay");
    expect(overlay.getAttribute("role")).toBe("status");
    expect(overlay.className).toContain("boot--full"); // nothing seen yet: first launch
    fireEvent.keyDown(window, { key: "Escape" });
    await act(async () => {
      await new Promise((r) => setTimeout(r, 500));
    });
    expect(screen.queryByTestId("boot-overlay")).toBeNull();
    expect(useBootStore.getState().finished).toBe(true);
  });

  it("regular start → short version; the version is remembered once ready", () => {
    window.localStorage.setItem(LAST_VERSION_KEY, APP_VERSION);
    const { rerender } = render(<BootHost ready={false} compact={false} />);
    expect(screen.getByTestId("boot-overlay").className).toContain("boot--short");
    rerender(<BootHost ready compact={false} />);
    expect(window.localStorage.getItem(LAST_VERSION_KEY)).toBe(APP_VERSION);
  });

  it("switched off: nothing is shown and Jarvis may greet right away", () => {
    window.localStorage.setItem(PREFS_KEY, JSON.stringify({ animation: "off", sound: "off", voice: false }));
    render(<BootHost ready={false} compact={false} />);
    expect(screen.queryByTestId("boot-overlay")).toBeNull();
    expect(useBootStore.getState().finished).toBe(true);
  });
});
