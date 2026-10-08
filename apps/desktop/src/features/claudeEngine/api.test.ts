import { describe, expect, it } from "vitest";

import { isValidAgentName, parseTools } from "./api";

describe("claude engine helpers", () => {
  it("validates agent file names like the core", () => {
    expect(isValidAgentName("code-reviewer")).toBe(true);
    expect(isValidAgentName("a_1")).toBe(true);
    expect(isValidAgentName("")).toBe(false);
    expect(isValidAgentName("-x")).toBe(false);
    expect(isValidAgentName("../evil")).toBe(false);
    expect(isValidAgentName("Has Space")).toBe(false);
  });

  it("parses comma or newline separated tools", () => {
    expect(parseTools("Read, Grep\nBash(npm test) ,")).toEqual(["Read", "Grep", "Bash(npm test)"]);
    expect(parseTools("  ")).toEqual([]);
  });
});
