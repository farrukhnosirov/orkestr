// Types the `contextPreview` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/contextPreview.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    contextPreview: typeof en;
  }
}
