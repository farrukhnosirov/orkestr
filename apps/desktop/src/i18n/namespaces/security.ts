// Types the `security` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/security.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    security: typeof en;
  }
}
