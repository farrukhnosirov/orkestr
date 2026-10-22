// Types the `remote` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/remote.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    remote: typeof en;
  }
}
