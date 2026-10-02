/**
 * Type-safe translation keys. Resources are typed per namespace from the
 * English JSON files through `ResourceNamespaceMap` augmentations — one file
 * per namespace in `src/i18n/namespaces/<ns>.ts`, so adding a namespace never
 * touches a shared file.
 */
import "i18next";

declare module "i18next" {
  interface CustomTypeOptions {
    defaultNS: "common";
    returnNull: false;
  }
}
