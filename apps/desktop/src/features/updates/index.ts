/** App updates (ADR-0048): Settings → Updates + the title-bar badge. */
import { Download } from "lucide-react";

import { t } from "../../i18n";
import type { SettingsSection } from "../settings/sections";
import { UpdatesSection } from "./UpdatesSection";

export { UpdateBadge } from "./UpdateBadge";
export { startUpdatesWatcher, useUpdatesStore } from "./store";
export { describeBootError } from "./bootError";

/** Settings section id (deep link `settings:updates`). */
export const UPDATES_SECTION_ID = "updates";

export const updatesSettingsSection: SettingsSection = {
  id: UPDATES_SECTION_ID,
  get title() {
    return t("updates:sectionTitle");
  },
  icon: Download,
  order: 950,
  component: UpdatesSection,
};
