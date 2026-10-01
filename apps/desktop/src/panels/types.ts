import type { LucideIcon } from "lucide-react";
import type { ComponentType } from "react";

/** Props every panel receives (contract §8). */
export interface PanelProps {
  /** Active project, or null when no project is open. */
  projectId: string | null;
}

export type PanelComponent = ComponentType<PanelProps>;

/** Icon component (lucide). */
export type PanelIcon = LucideIcon;

/** Registry entry of a dockable panel (§14). */
export interface PanelDefinition {
  /** Stable id, also the dockview component name. */
  id: string;
  /** Tab title. */
  title: string;
  /** One-line purpose. */
  description: string;
  /** Icon. */
  icon: PanelIcon;
  /** Roadmap phase (§61) in which the panel becomes functional; 0 = available now. */
  phase: number;
  /** The component. */
  component: PanelComponent;
}
