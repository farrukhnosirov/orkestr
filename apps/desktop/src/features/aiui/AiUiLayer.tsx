/**
 * Mounted once by the main window: the AI-editable UI runtime (stored UI
 * configuration + live preview applied to the DOM) and the proposal card.
 */
import { UiPatchCard } from "./UiPatchCard";
import { useAiUiRuntime } from "./useAiUiRuntime";

export function AiUiLayer({ connected }: { connected: boolean }) {
  useAiUiRuntime(connected);
  return <UiPatchCard />;
}
