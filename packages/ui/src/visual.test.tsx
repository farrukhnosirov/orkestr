import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { Illustration, Skeleton, SkeletonLines, type IllustrationKind } from "./visual";

afterEach(() => cleanup());

describe("visual primitives", () => {
  it("skeletons are decorative inside a busy status", () => {
    render(<SkeletonLines lines={3} label="Loading" />);
    const status = screen.getByRole("status", { name: "Loading" });
    expect(status.getAttribute("aria-busy")).toBe("true");
    expect(status.querySelectorAll(".ok-skeleton[aria-hidden='true']").length).toBe(3);
    const { container } = render(<Skeleton shape="circle" width={24} height={24} />);
    expect(container.querySelector(".ok-skeleton--circle")).toBeTruthy();
  });

  it("illustrations render procedural SVG with unique gradient ids", () => {
    const kinds: IllustrationKind[] = ["orbit", "constellation", "inbox", "signal"];
    const { container } = render(
      <>
        {kinds.map((k) => (
          <Illustration key={k} kind={k} />
        ))}
      </>,
    );
    const svgs = container.querySelectorAll("svg.ok-illustration");
    expect(svgs.length).toBe(4);
    const ids = [...container.querySelectorAll("linearGradient")].map((g) => g.id);
    expect(new Set(ids).size).toBe(ids.length);
    expect([...svgs].every((s) => s.getAttribute("aria-hidden") === "true")).toBe(true);
  });
});
