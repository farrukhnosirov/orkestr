// Types the `commands` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/commands.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    commands: typeof en;
  }
}
