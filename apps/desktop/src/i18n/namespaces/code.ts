// Types the `code` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/code.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    code: typeof en;
  }
}
