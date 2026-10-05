/**
 * Mission Control view state: full-screen mode and the widget arrangement
 * (persisted through the pluggable `ArrangementStorage`).
 */
import { create } from "zustand";

import {
  ARRANGEMENT_KEY,
  type Arrangement,
  arrangementStorage,
  cycleSize,
  defaultArrangement,
  moveWidget,
  moveWidgetBy,
  parseArrangement,
  serializeArrangement,
  setHidden,
} from "./arrangement";
import { getWidget, widgets } from "./widgets/registry";

export interface MissionControlState {
  /** Mission Control covers the whole workspace (Ctrl+Shift+D). */
  fullscreen: boolean;
  /** Layout editing (size / hide controls visible). */
  editing: boolean;
  arrangement: Arrangement;
  setFullscreen: (on: boolean) => void;
  toggleFullscreen: () => void;
  setEditing: (on: boolean) => void;
  move: (id: string, targetId: string) => void;
  moveBy: (id: string, delta: number) => void;
  cycle: (id: string) => void;
  hide: (id: string, hidden: boolean) => void;
  reset: () => void;
  /** Re-reads the arrangement from storage (after a storage rebind). */
  reload: () => void;
}

function load(): Arrangement {
  return parseArrangement(arrangementStorage().load(ARRANGEMENT_KEY), widgets());
}

function persist(a: Arrangement): Arrangement {
  arrangementStorage().save(ARRANGEMENT_KEY, serializeArrangement(a));
  return a;
}

export const useMissionControlStore = create<MissionControlState>()((set, get) => ({
  fullscreen: false,
  editing: false,
  arrangement: load(),
  setFullscreen: (fullscreen) => set({ fullscreen }),
  toggleFullscreen: () => set({ fullscreen: !get().fullscreen }),
  setEditing: (editing) => set({ editing }),
  move: (id, targetId) => set({ arrangement: persist(moveWidget(get().arrangement, id, targetId)) }),
  moveBy: (id, delta) => set({ arrangement: persist(moveWidgetBy(get().arrangement, id, delta)) }),
  cycle: (id) => {
    const def = getWidget(id);
    if (def) set({ arrangement: persist(cycleSize(get().arrangement, id, def.sizes)) });
  },
  hide: (id, hidden) => set({ arrangement: persist(setHidden(get().arrangement, id, hidden)) }),
  reset: () => set({ arrangement: persist(defaultArrangement(widgets())) }),
  reload: () => set({ arrangement: load() }),
}));
