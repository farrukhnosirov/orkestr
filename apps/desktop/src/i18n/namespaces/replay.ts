// Types the `replay` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/replay.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    replay: typeof en;
  }
}
