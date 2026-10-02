// Types the `placeholders` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/placeholders.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    placeholders: typeof en;
  }
}
