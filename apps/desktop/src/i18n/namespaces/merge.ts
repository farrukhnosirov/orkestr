// Types the `merge` namespace (Git panel → Merge Manager) from its English source.
import type en from "../../locales/en/merge.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    merge: typeof en;
  }
}
