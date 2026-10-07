// Types the `claudeCode` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/claudeCode.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    claudeCode: typeof en;
  }
}
