// Types the `consensus` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/consensus.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    consensus: typeof en;
  }
}
