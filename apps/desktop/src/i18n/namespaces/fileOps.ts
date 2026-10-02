// Types the `fileOps` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/fileOps.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    fileOps: typeof en;
  }
}
