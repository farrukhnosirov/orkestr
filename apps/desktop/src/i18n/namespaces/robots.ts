// Types the `robots` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/robots.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    robots: typeof en;
  }
}
