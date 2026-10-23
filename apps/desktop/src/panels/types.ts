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

/**
 * Window Library category of a panel (Wave 8). Order here is the order of
 * the category rail in the library.
 */
export const PANEL_CATEGORIES = ["work", "ai", "code", "quality", "ops", "monitoring", "ecosystem", "system"] as const;
export type PanelCategory = (typeof PANEL_CATEGORIES)[number];

/** Registry entry of a dockable panel (§14). */
export interface PanelDefinition {
  /** Stable id, also the dockview component name. */
  id: string;
  /** Tab title (localized: registries define it as a getter over `t()`). */
  readonly title: string;
  /** One-line purpose (localized getter). */
  readonly description: string;
  /** Icon. */
  icon: PanelIcon;
  /** Window Library category. */
  category: PanelCategory;
  /** Window Library card text: 1–2 lines (localized getter, `windowLibrary:panels.<id>.summary`). */
  readonly summary: string;
  /** Window Library detail-pane text (localized getter). */
  readonly details: string;
  /** Localized search tags (getter; English and Russian are both searched by the library). */
  readonly tags: readonly string[];
  /** Roadmap phase (§61) in which the panel becomes functional; 0 = available now. */
  phase: number;
  /** The component. */
  component: PanelComponent;
}
