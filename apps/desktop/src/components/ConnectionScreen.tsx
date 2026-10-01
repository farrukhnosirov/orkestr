import { Button, Spinner } from "@orkestr/ui";
import { AlertTriangle } from "lucide-react";

import { useAppStore } from "../store/app";
import { Logo } from "./Logo";

/** Splash shown until the first successful connection to the core. */
export function ConnectionScreen({ onRetry }: { onRetry: () => void }) {
  const connection = useAppStore((s) => s.connection);
  const failed = connection.state === "closed";
  return (
    <div className="splash" data-tauri-drag-region>
      <div className="splash__orb">
        <Logo size={72} />
      </div>
      <h1 className="splash__title">Orkestr</h1>
      <p className="splash__subtitle">AI Development OS</p>
      {failed ? (
        <div className="splash__error" role="alert">
          <AlertTriangle size={16} />
          <span>{connection.error ?? "Could not connect to the Orkestr core."}</span>
        </div>
      ) : (
        <div className="splash__status">
          <Spinner size={16} />
          <span>{connection.state === "authenticating" ? "Authenticating…" : "Starting the core…"}</span>
        </div>
      )}
      {failed && (
        <Button variant="primary" onClick={onRetry}>
          Retry
        </Button>
      )}
    </div>
  );
}
