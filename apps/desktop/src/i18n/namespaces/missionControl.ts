// Types the `missionControl` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/missionControl.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    missionControl: typeof en;
  }
}
