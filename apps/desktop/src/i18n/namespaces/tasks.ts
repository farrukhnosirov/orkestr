// Types the `tasks` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/tasks.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    tasks: typeof en;
  }
}
