/** The HUD's voice bridge survives StrictMode effect re-runs (closed channel used to throw and blank the HUD). */
import { act, cleanup, render } from "@testing-library/react";
import { StrictMode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { useRemoteVoice } from "./voiceBridge";

/** Behaves like the browser's channel: posting after `close()` throws. */
class FakeChannel {
  static all: FakeChannel[] = [];
  closed = false;
  posted: unknown[] = [];
  onmessage: ((e: MessageEvent) => void) | null = null;
  constructor(public name: string) {
    FakeChannel.all.push(this);
  }
  postMessage(msg: unknown) {
    if (this.closed) throw new DOMException("Channel is closed", "InvalidStateError");
    this.posted.push(msg);
  }
  close() {
    this.closed = true;
  }
}

let toggle: (() => void) | null = null;
function Probe() {
  toggle = useRemoteVoice().toggle;
  return null;
}

describe("useRemoteVoice", () => {
  const original = globalThis.BroadcastChannel;
  beforeEach(() => {
    FakeChannel.all = [];
    (globalThis as { BroadcastChannel: unknown }).BroadcastChannel = FakeChannel;
  });
  afterEach(() => {
    cleanup();
    (globalThis as { BroadcastChannel: unknown }).BroadcastChannel = original;
    toggle = null;
  });

  it("does not post to a closed channel when StrictMode re-runs the effect", () => {
    const errors = vi.spyOn(console, "error").mockImplementation(() => undefined);
    expect(() =>
      render(
        <StrictMode>
          <Probe />
        </StrictMode>,
      ),
    ).not.toThrow();
    const open = FakeChannel.all.filter((c) => !c.closed);
    expect(open).toHaveLength(1);
    expect(open[0]?.posted).toEqual([{ type: "hello" }]);
    act(() => toggle?.());
    expect(open[0]?.posted).toEqual([{ type: "hello" }, { type: "toggle" }]);
    errors.mockRestore();
  });

  it("closes its channel on unmount and a late toggle is a no-op", () => {
    const view = render(<Probe />);
    const late = toggle;
    view.unmount();
    expect(FakeChannel.all.every((c) => c.closed)).toBe(true);
    expect(() => late?.()).not.toThrow();
  });
});
