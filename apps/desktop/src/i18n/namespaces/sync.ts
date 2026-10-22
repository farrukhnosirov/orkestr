// Types the `sync` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/sync.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    sync: typeof en;
  }
}
