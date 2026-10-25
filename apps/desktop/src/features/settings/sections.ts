/**
 * Settings section registry. Each feature contributes a section by appending
 * an entry here (one line + one import). The Settings view renders them in order.
 */
import { Info, MonitorCog, SlidersHorizontal, type LucideIcon } from "lucide-react";
import type { ComponentType } from "react";

import { t } from "../../i18n";
import { editorSettingsSection } from "../extensions";
import { visualSettingsSection } from "../performance/section";
import { managerSettingsSection, providersSettingsSection, sourcesSettingsSection } from "../providers";
import { qualitySettingsSection } from "../quality";
import { updatesSettingsSection } from "../updates";
import { soundSettingsSection, voiceSettingsSection } from "../voice";
import { vscodeSettingsSection } from "../vscode";
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
settingsSections.push(providersSettingsSection, managerSettingsSection, sourcesSettingsSection);
settingsSections.push(qualitySettingsSection);
// Phase 3: Interface (Performance Mode, density, HUD, UI change history).
settingsSections.push(visualSettingsSection);
// voice-sound (Phase 3)
settingsSections.push(voiceSettingsSection, soundSettingsSection);
// Embedded VS Code
settingsSections.push(vscodeSettingsSection);
// VS Code extensions: editor theme, icon theme, language servers (ADR-0025)
settingsSections.push(editorSettingsSection);
// App updates + data backups (ADR-0048)
settingsSections.push(updatesSettingsSection);

/** Sections in display order (stable for equal `order`). */
export function orderedSections(list: readonly SettingsSection[] = settingsSections): SettingsSection[] {
  return list
    .map((s, i) => ({ s, i }))
    .sort((a, b) => a.s.order - b.s.order || a.i - b.i)
    .map((x) => x.s);
}
