// Types the `notifications` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/notifications.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    notifications: typeof en;
  }
}
