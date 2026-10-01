/**
 * Settings section registry. Each feature contributes a section by appending
 * an entry here (one line + one import). The Settings view renders them in order.
 */
import type { LucideIcon } from "lucide-react";
import type { ComponentType } from "react";

export interface SettingsSectionProps {
  /** Active project, or null for global settings. */
  projectId: string | null;
}

export interface SettingsSection {
  /** Stable id (used in deep links, e.g. `settings:providers`). */
  id: string;
  title: string;
  icon: LucideIcon;
  /** Lower comes first. */
  order: number;
  component: ComponentType<SettingsSectionProps>;
}

export const settingsSections: SettingsSection[] = [];
