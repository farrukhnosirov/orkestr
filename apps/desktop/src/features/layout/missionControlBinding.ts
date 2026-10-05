/**
 * Mission Control's widget arrangement lives with the active layout of this
 * device (`LayoutDocument.widgets.missionControl`, stored in the core by the
 * layout area): each layout preset keeps its own dashboard, and switching
 * layouts switches the arrangement. A browser-local arrangement from before
 * this binding is used once as the starting point.
 */
import { setArrangementStorage, type ArrangementStorage } from "../../panels/MissionControl/arrangement";
import { useMissionControlStore } from "../../panels/MissionControl/store";
import { getWidgetState, onWidgetStateReplaced, setWidgetState } from "./controller";

/** Key of Mission Control's entry in `LayoutDocument.widgets` (its panel id). */
export const MISSION_CONTROL_WIDGET_KEY = "missionControl";

function legacy(key: string): string | null {
  try {
    return window.localStorage.getItem(key);
  } catch {
    return null;
  }
}

export const layoutArrangementStorage: ArrangementStorage = {
  load: (key) => getWidgetState(MISSION_CONTROL_WIDGET_KEY) ?? legacy(key),
  save: (_key, value) => setWidgetState(MISSION_CONTROL_WIDGET_KEY, value),
};

let bound = false;

/** Binds the storage once (main window). */
export function bindMissionControlToLayout(): void {
  if (bound) return;
  bound = true;
  setArrangementStorage(layoutArrangementStorage);
  onWidgetStateReplaced(() => useMissionControlStore.getState().reload());
  useMissionControlStore.getState().reload();
}
