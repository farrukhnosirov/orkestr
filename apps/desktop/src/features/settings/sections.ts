/**
 * Settings section registry. Each feature contributes a section by appending
 * an entry here (one line + one import). The Settings view renders them in order.
 */
import { Info, MonitorCog, SlidersHorizontal, type LucideIcon } from "lucide-react";
import type { ComponentType } from "react";

import { managerSettingsSection, providersSettingsSection } from "../providers";
import { AboutSection } from "./sections/AboutSection";
import { GeneralSection } from "./sections/GeneralSection";
import { SystemSection } from "./sections/SystemSection";

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

export const settingsSections: SettingsSection[] = [
  { id: "general", title: "General", icon: SlidersHorizontal, order: 0, component: GeneralSection },
  { id: "system", title: "System", icon: MonitorCog, order: 900, component: SystemSection },
  { id: "about", title: "About", icon: Info, order: 1000, component: AboutSection },
];
settingsSections.push(providersSettingsSection, managerSettingsSection);

/** Sections in display order (stable for equal `order`). */
export function orderedSections(list: readonly SettingsSection[] = settingsSections): SettingsSection[] {
  return list
    .map((s, i) => ({ s, i }))
    .sort((a, b) => a.s.order - b.s.order || a.i - b.i)
    .map((x) => x.s);
}
