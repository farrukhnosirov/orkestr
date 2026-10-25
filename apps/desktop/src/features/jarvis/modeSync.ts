/**
 * Keeps the voice store's `assistantMode` (the Jarvis circle's mode, in the
 * orb and the in-app HUD — ADR-0052) in step with the conversation mode
 * (ADR-0049) and cursor capture mode (ADR-0050): cursor while the AI drives
 * the cursor, else chat / command.
 */
import { useCursorStore } from "../cursor/store";
import type { JarvisMode } from "../voice/jarvis/model";
import { useVoiceStore } from "../voice/store";
import { useJarvisStore } from "./store";

/** The circle's mode from the two stores. */
export function circleMode(cursorActive: boolean, conversation: string): JarvisMode {
  if (cursorActive) return "cursor";
  return conversation === "chat" ? "chat" : "command";
}

/** Starts syncing; returns the unsubscribe. */
export function startJarvisModeSync(): () => void {
  const apply = () => {
    const mode = circleMode(useCursorStore.getState().active, useJarvisStore.getState().mode);
    if (useVoiceStore.getState().assistantMode !== mode) useVoiceStore.getState().setRuntime({ assistantMode: mode });
  };
  apply();
  const offCursor = useCursorStore.subscribe(apply);
  const offJarvis = useJarvisStore.subscribe(apply);
  return () => {
    offCursor();
    offJarvis();
  };
}
