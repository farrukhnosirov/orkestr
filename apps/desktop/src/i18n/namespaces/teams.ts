// Types the `teams` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/teams.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    teams: typeof en;
  }
}
