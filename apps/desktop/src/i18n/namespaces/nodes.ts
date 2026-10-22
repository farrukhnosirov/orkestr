// Types the `nodes` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/nodes.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    nodes: typeof en;
  }
}
