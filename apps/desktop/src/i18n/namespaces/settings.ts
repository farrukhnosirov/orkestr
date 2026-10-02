// Types the `settings` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/settings.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    settings: typeof en;
  }
}
