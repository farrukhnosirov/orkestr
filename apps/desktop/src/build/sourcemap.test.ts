import { describe, expect, it } from "vitest";

import { shouldEmitSourcemap } from "./sourcemap";

describe("shouldEmitSourcemap", () => {
  it("does not emit maps for a plain production build", () => {
    expect(shouldEmitSourcemap({})).toBe(false);
  });

  it('does not emit maps for a Tauri release build (TAURI_ENV_DEBUG="false")', () => {
    expect(shouldEmitSourcemap({ TAURI_ENV_DEBUG: "false" })).toBe(false);
    expect(shouldEmitSourcemap({ TAURI_ENV_DEBUG: "" })).toBe(false);
  });

  it("emits maps for a Tauri debug build", () => {
    expect(shouldEmitSourcemap({ TAURI_ENV_DEBUG: "true" })).toBe(true);
  });

  it("emits maps on explicit opt-in", () => {
    expect(shouldEmitSourcemap({ ORKESTR_SOURCEMAP: "1" })).toBe(true);
    expect(shouldEmitSourcemap({ ORKESTR_SOURCEMAP: "TRUE" })).toBe(true);
    expect(shouldEmitSourcemap({ ORKESTR_SOURCEMAP: "0" })).toBe(false);
  });
});
