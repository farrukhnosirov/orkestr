// Types the `assistant` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/assistant.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    assistant: typeof en;
  }
}
