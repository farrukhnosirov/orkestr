/** The active dock tab stays visible after a group's tab strip narrows (restore after maximize, resize). */
import { describe, expect, it } from "vitest";

import { revealActiveTabs, revealScroll } from "./tabReveal";

describe("revealScroll", () => {
  it("leaves a visible tab alone", () => {
    expect(revealScroll({ tabStart: 0, tabSize: 100, scroll: 0, visible: 200 })).toBeNull();
  });

  it("scrolls to an active tab pushed past the right edge (restore after maximize)", () => {
    // Measured case: strip 101 px wide, active tab at 107..214, scrollLeft 0.
    expect(revealScroll({ tabStart: 107, tabSize: 107, scroll: 0, visible: 101 })).toBe(107);
    expect(revealScroll({ tabStart: 107, tabSize: 60, scroll: 0, visible: 101 })).toBe(66);
  });

  it("scrolls back to a tab left of the visible area", () => {
    expect(revealScroll({ tabStart: 20, tabSize: 50, scroll: 120, visible: 100 })).toBe(20);
  });
});

describe("revealActiveTabs", () => {
  it("scrolls each strip to its active tab", () => {
    document.body.innerHTML = `
      <div class="dv-tabs-container" id="a"><div class="dv-tab dv-inactive-tab"></div><div class="dv-tab dv-active-tab" id="t"></div></div>`;
    const strip = document.getElementById("a")!;
    const tab = document.getElementById("t")!;
    Object.defineProperty(tab, "offsetLeft", { value: 107 });
    Object.defineProperty(tab, "offsetWidth", { value: 107 });
    Object.defineProperty(strip, "clientWidth", { value: 101 });
    strip.scrollLeft = 0;
    revealActiveTabs(document);
    expect(strip.scrollLeft).toBe(107);
    document.body.innerHTML = "";
  });
});
