// Types the `quality` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/quality.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    quality: typeof en;
  }
}
