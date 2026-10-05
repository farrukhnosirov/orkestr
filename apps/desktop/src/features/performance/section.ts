/** Settings → Interface section entry (registered in `settings/sections.ts`). */
import { MonitorSmartphone } from "lucide-react";

import { t } from "../../i18n";
import type { SettingsSection } from "../settings/sections";
import { VISUAL_SECTION_ID } from "./PerfChip";
import { VisualSettings } from "./VisualSettings";

export const visualSettingsSection: SettingsSection = {
  id: VISUAL_SECTION_ID,
  get title() {
    return t("performance:sectionTitle");
  },
  icon: MonitorSmartphone,
  order: 10,
  component: VisualSettings,
};
