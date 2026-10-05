// Types the `performance` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/performance.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    performance: typeof en;
  }
}
