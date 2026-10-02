// Types the `coreText` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/coreText.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    coreText: typeof en;
  }
}
