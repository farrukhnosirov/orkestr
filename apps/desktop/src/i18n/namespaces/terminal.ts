// Types the `terminal` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/terminal.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    terminal: typeof en;
  }
}
