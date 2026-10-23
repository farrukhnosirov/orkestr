// Types the `orb` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/orb.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    orb: typeof en;
  }
}
