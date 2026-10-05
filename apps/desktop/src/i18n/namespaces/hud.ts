// Types the `hud` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/hud.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    hud: typeof en;
  }
}
