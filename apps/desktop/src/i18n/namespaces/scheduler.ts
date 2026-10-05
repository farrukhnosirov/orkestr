// Types the `scheduler` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/scheduler.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    scheduler: typeof en;
  }
}
