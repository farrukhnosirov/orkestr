// Types the `vscode` namespace from its English source (see ../i18next.d.ts).
import type en from "../../locales/en/vscode.json";

declare module "i18next" {
  interface ResourceNamespaceMap {
    vscode: typeof en;
  }
}
