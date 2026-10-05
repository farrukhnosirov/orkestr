// Types the `aiui` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/aiui.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    aiui: typeof en;
  }
}
