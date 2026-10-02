// Types the `agents` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/agents.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    agents: typeof en;
  }
}
