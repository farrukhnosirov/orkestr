/**
 * Infrastructure monitor section of the Monitoring panel: health checks, log
 * sources, alerts (ADR-0042). **Owner: WP5.** Step-0 skeleton: renders a
 * short "being built" note below the local telemetry.
 */
import { useT } from "../../../i18n";

export function InfraMonitor() {
  const { t } = useT("monitorInfra");
  return (
    <section className="mon-card" aria-label={t("stub.title")}>
      <strong>{t("stub.title")}</strong>
      <p className="mon-muted">{t("stub.description")}</p>
    </section>
  );
}
