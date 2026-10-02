// Types the `costs` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/costs.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    costs: typeof en;
  }
}
