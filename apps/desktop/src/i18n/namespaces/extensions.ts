// Types the `extensions` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/extensions.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    extensions: typeof en;
  }
}
