// Types the `common` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/common.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    common: typeof en;
  }
}
