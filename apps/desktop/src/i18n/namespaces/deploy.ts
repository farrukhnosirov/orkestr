// Types the `deploy` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/deploy.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    deploy: typeof en;
  }
}
