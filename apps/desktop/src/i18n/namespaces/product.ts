// Types the `product` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/product.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    product: typeof en;
  }
}
