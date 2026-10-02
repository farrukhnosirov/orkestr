// Types the `shell` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/shell.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    shell: typeof en;
  }
}
