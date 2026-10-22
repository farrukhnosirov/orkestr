import { useTranslation } from "react-i18next";

/** Placeholder shell; login, pairing and panels arrive with the web-client package. */
export function App() {
  const { t } = useTranslation("common");
  return (
    <main>
      <h1>{t("app.title")}</h1>
      <p>{t("app.placeholder")}</p>
    </main>
  );
}
