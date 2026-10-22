// Types the `windowLibrary` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/windowLibrary.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    windowLibrary: typeof en;
  }
}
