// Types the `cursor` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/cursor.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    cursor: typeof en;
  }
}
