/**
 * Main-window state of the robots feature: the robots document (`robots.config`,
 * cached from the core), robots the user hid this session and the Robot
 * Designer's target. Business data comes from the core over RPC only.
 */
import type { RobotDesign, RobotsConfig, SetRobotsConfigParams } from "@orkestr/protocol";
import { create } from "zustand";

import { maybeClient } from "../../core/client";
import { describeError } from "../../i18n/errors";
import { useSettingsStore } from "../settings/settingsStore";
import { robotShell } from "./shell";

/** Designer target: the default design or one template's design. */
export type DesignerTarget = { kind: "default" } | { kind: "template"; templateId: string };

export interface RobotsState {
  config: RobotsConfig | null;
  loading: boolean;
  saving: boolean;
  error: string | null;
  /** Agent ids hidden from the desktop until the app restarts. */
  hidden: ReadonlySet<string>;
  target: DesignerTarget;

  load: () => Promise<void>;
  patch: (p: SetRobotsConfigParams) => Promise<boolean>;
  saveDesign: (target: DesignerTarget, design: RobotDesign | null) => Promise<boolean>;
  hide: (agentId: string) => void;
  unhideAll: () => void;
  setTarget: (target: DesignerTarget) => void;
}

export const useRobotsStore = create<RobotsState>()((set, get) => ({
  config: null,
  loading: false,
  saving: false,
  error: null,
  hidden: new Set<string>(),
  target: { kind: "default" },

  load: async () => {
    const c = maybeClient();
    if (!c) return;
    set({ loading: true, error: null });
    try {
      const { config } = await c.call("robots.config", {});
      set({ config, loading: false });
    } catch (e) {
      set({ loading: false, error: describeError(e) });
    }
  },

  patch: async (p) => {
    const c = maybeClient();
    if (!c) return false;
    set({ saving: true, error: null });
    try {
      const { config } = await c.call("robots.setConfig", p);
      set({ config, saving: false });
      // Robot windows re-read their design.
      robotShell.broadcastConfigChanged().catch((e: unknown) => console.warn("robots config broadcast", e));
      return true;
    } catch (e) {
      set({ saving: false, error: describeError(e) });
      return false;
    }
  },

  saveDesign: (target, design) => {
    if (target.kind === "default") {
      return design ? get().patch({ defaultDesign: design }) : Promise.resolve(false);
    }
    return get().patch({ design: { templateId: target.templateId, design } });
  },

  hide: (agentId) => {
    const hidden = new Set(get().hidden);
    hidden.add(agentId);
    set({ hidden });
  },
  unhideAll: () => set({ hidden: new Set<string>() }),
  setTarget: (target) => set({ target }),
}));

/** Opens Settings → Robots on a template's design (Agents → Templates "Robot" button). */
export function openRobotDesigner(templateId: string | null): void {
  useRobotsStore.getState().setTarget(templateId ? { kind: "template", templateId } : { kind: "default" });
  useSettingsStore.getState().openSettings("robots");
}
