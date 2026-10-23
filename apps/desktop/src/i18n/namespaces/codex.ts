// Types the `codex` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/codex.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    codex: typeof en;
  }
}
