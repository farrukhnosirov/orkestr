// Types the `boot` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/boot.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    boot: typeof en;
  }
}
