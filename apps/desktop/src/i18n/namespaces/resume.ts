// Types the `resume` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/resume.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    resume: typeof en;
  }
}
