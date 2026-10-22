// Types the `contextMenu` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/contextMenu.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    contextMenu: typeof en;
  }
}
