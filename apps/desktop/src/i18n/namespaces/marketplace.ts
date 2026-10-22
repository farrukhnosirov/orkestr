// Types the `marketplace` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/marketplace.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    marketplace: typeof en;
  }
}
