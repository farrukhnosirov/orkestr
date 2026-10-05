/**
 * Settings section registry. Each feature contributes a section by appending
 * an entry here (one line + one import). The Settings view renders them in order.
 */
import { Info, MonitorCog, SlidersHorizontal, type LucideIcon } from "lucide-react";
import type { ComponentType } from "react";

import { t } from "../../i18n";
import { visualSettingsSection } from "../performance/section";
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
  /** Localized title — define it as a getter over `t()` so it follows language switches. */
  readonly title: string;
  icon: LucideIcon;
  /** Lower comes first. */
  order: number;
  component: ComponentType<SettingsSectionProps>;
}

export const settingsSections: SettingsSection[] = [
  {
    id: "general",
    get title() {
      return t("shell:settings.sectionTitles.general");
    },
    icon: SlidersHorizontal,
    order: 0,
    component: GeneralSection,
  },
  {
    id: "system",
    get title() {
      return t("shell:settings.sectionTitles.system");
    },
    icon: MonitorCog,
    order: 900,
    component: SystemSection,
  },
  {
    id: "about",
    get title() {
      return t("shell:settings.sectionTitles.about");
    },
    icon: Info,
    order: 1000,
    component: AboutSection,
  },
];
settingsSections.push(providersSettingsSection, managerSettingsSection);
// Phase 3: Interface (Performance Mode, density, HUD, UI change history).
settingsSections.push(visualSettingsSection);

/** Sections in display order (stable for equal `order`). */
export function orderedSections(list: readonly SettingsSection[] = settingsSections): SettingsSection[] {
  return list
    .map((s, i) => ({ s, i }))
    .sort((a, b) => a.s.order - b.s.order || a.i - b.i)
    .map((x) => x.s);
}
