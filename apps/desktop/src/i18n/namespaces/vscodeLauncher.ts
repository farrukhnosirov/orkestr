// Types the `vscodeLauncher` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/vscodeLauncher.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    vscodeLauncher: typeof en;
  }
}
