// Types the `architecture` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/architecture.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    architecture: typeof en;
  }
}
