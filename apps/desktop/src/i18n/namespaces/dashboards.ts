// Types the `dashboards` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/dashboards.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    dashboards: typeof en;
  }
}
