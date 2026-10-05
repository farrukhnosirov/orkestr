// Types the `voice` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/voice.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    voice: typeof en;
  }
}
