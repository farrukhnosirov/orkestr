// Types the `detective` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/detective.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    detective: typeof en;
  }
}
