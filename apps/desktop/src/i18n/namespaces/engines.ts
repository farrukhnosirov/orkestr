// Types the `engines` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/engines.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    engines: typeof en;
  }
}
