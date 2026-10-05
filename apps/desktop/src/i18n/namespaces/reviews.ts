// Types the `reviews` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/reviews.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    reviews: typeof en;
  }
}
