/**
 * VS Code extensions in Orkestr's own editor (ADR-0025): registry store,
 * file icon themes and the Settings → Editor section.
 */
import { SquareCode } from "lucide-react";

import { t } from "../../i18n";
import type { SettingsSection } from "../settings/sections";
import { EditorSettings } from "./EditorSettings";

export const editorSettingsSection: SettingsSection = {
  id: "editor",
  get title() {
    return t("extensions:settings.title");
  },
  icon: SquareCode,
  order: 20,
  component: EditorSettings,
};
