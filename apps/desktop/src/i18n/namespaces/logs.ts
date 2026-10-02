// Types the `logs` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/logs.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    logs: typeof en;
  }
}
