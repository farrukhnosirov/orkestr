// Types the `onboarding` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/onboarding.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    onboarding: typeof en;
  }
}
