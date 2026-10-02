// Types the `manager` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/manager.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    manager: typeof en;
  }
}
