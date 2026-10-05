// Types the `theme` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/theme.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    theme: typeof en;
  }
}
