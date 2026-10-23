// Types the `updates` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/updates.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    updates: typeof en;
  }
}
