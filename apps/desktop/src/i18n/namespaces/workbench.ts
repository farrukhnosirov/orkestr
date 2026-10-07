// Types the `workbench` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/workbench.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    workbench: typeof en;
  }
}
