// Types the `events` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/events.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    events: typeof en;
  }
}
