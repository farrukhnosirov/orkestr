// Types the `git` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/git.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    git: typeof en;
  }
}
