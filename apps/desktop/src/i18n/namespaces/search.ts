// Types the `search` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/search.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    search: typeof en;
  }
}
