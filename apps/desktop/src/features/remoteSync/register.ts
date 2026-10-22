/**
 * Registers the Remote & Sync settings section (ADR-0030/0031).
 * Imported once for its side effects.
 */
import { RefreshCcwDot } from "lucide-react";

import { t } from "../../i18n";
import { settingsSections } from "../settings/sections";
import { RemoteSyncSettings } from "./RemoteSyncSettings";

let registered = false;

export function registerRemoteSyncFeature(): void {
  if (registered) return;
  registered = true;
  settingsSections.push({
    id: "remoteSync",
    get title() {
      return t("sync:section");
    },
    icon: RefreshCcwDot,
    order: 43,
    component: RemoteSyncSettings,
  });
}
