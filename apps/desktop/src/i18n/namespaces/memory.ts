// Types the `memory` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/memory.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    memory: typeof en;
  }
}
