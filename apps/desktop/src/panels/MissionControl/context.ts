/**
 * Shared state of one Mission Control surface (panel or full-screen view):
 * the aggregated snapshot is fetched once and handed to every widget.
 */
import type { DashboardSnapshot } from "@orkestr/protocol";
import { createContext, useContext } from "react";

import type { RpcQuery } from "../../lib/rpcQuery";
import type { WidgetSize } from "./arrangement";
import type { VisualBudget } from "./visual";

export interface MissionContextValue {
  snapshot: RpcQuery<DashboardSnapshot>;
  budget: VisualBudget;
  /** Project the graph widget follows (defaults to the active / most active project). */
  focusProjectId: string | null;
  setFocusProjectId: (id: string | null) => void;
}

export const MissionContext = createContext<MissionContextValue | null>(null);

export function useMission(): MissionContextValue {
  const ctx = useContext(MissionContext);
  if (!ctx) throw new Error("Mission Control widgets must render inside <MissionContext.Provider>");
  return ctx;
}

/** Props every widget receives. */
export interface WidgetProps {
  size: WidgetSize;
}
