// Types the `jarvis` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/jarvis.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    jarvis: typeof en;
  }
}
