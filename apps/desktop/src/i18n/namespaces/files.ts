// Types the `files` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/files.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    files: typeof en;
  }
}
