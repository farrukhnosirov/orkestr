import { Button, Spinner } from "@orkestr/ui";
import { AlertTriangle } from "lucide-react";

import { describeBootError } from "../features/updates";
import { useT } from "../i18n";
import { useAppStore } from "../store/app";
import { Logo } from "./Logo";

/** Splash shown until the first successful connection to the core. */
export function ConnectionScreen({ onRetry }: { onRetry: () => void }) {
  const connection = useAppStore((s) => s.connection);
  const failed = connection.state === "closed";
  const { t } = useT(["shell", "common"]);
  return (
    <div className="splash" data-tauri-drag-region>
      <div className="splash__orb">
        <Logo size={72} />
      </div>
      <h1 className="splash__title">Orkestr</h1>
      <p className="splash__subtitle">{t("common:tagline")}</p>
      {failed ? (
        <div className="splash__error" role="alert">
          <AlertTriangle size={16} />
          <span>{describeBootError(connection.error) ?? connection.error ?? t("shell:connection.failed")}</span>
        </div>
      ) : (
        <div className="splash__status">
          <Spinner size={16} />
          <span>{connection.state === "authenticating" ? t("shell:connection.authenticating") : t("shell:connection.starting")}</span>
        </div>
      )}
      {failed && (
        <Button variant="primary" onClick={onRetry}>
          {t("common:actions.retry")}
        </Button>
      )}
    </div>
  );
}
