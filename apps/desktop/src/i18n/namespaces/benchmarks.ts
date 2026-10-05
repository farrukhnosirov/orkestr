// Types the `benchmarks` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/benchmarks.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    benchmarks: typeof en;
  }
}
