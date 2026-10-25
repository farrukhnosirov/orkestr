import { describe, expect, it } from "vitest";

import { useCursorStore } from "../cursor/store";
import { useVoiceStore } from "../voice/store";
import { circleMode, startJarvisModeSync } from "./modeSync";
import { useJarvisStore } from "./store";

describe("Jarvis circle mode sync", () => {
  it("maps cursor / conversation onto the circle's mode", () => {
    expect(circleMode(true, "chat")).toBe("cursor");
    expect(circleMode(false, "chat")).toBe("chat");
    expect(circleMode(false, "commands")).toBe("command");
  });

  it("follows the stores", () => {
    const off = startJarvisModeSync();
    useJarvisStore.getState().setMode("chat");
    expect(useVoiceStore.getState().assistantMode).toBe("chat");
    useCursorStore.getState().patch({ active: true });
    expect(useVoiceStore.getState().assistantMode).toBe("cursor");
    useCursorStore.getState().patch({ active: false });
    useJarvisStore.getState().setMode("commands");
    expect(useVoiceStore.getState().assistantMode).toBe("command");
    off();
  });
});
