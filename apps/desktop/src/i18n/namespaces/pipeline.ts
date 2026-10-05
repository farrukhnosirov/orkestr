// Types the `pipeline` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/pipeline.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    pipeline: typeof en;
  }
}
