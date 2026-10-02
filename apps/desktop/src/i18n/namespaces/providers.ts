// Types the `providers` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/providers.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    providers: typeof en;
  }
}
