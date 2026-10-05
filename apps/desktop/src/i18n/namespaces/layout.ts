// Types the `layout` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/layout.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    layout: typeof en;
  }
}
