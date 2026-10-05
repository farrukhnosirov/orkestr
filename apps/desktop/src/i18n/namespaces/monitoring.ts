// Types the `monitoring` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/monitoring.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    monitoring: typeof en;
  }
}
