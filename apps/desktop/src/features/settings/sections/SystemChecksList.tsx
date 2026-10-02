import type { CheckStatus, SystemCheck } from "@orkestr/protocol";
import { Badge, type Tone } from "@orkestr/ui";
import { CheckCircle2, CircleSlash, XCircle, AlertTriangle } from "lucide-react";

export const CHECK_TONE: Record<CheckStatus, Tone> = {
  ok: "success",
  warning: "warning",
  error: "danger",
  skipped: "neutral",
};

const LABEL: Record<CheckStatus, string> = { ok: "Ready", warning: "Warning", error: "Action needed", skipped: "Skipped" };

export function CheckIcon({ status, size = 16 }: { status: CheckStatus; size?: number }) {
  switch (status) {
    case "ok":
      return <CheckCircle2 size={size} className="chk-icon chk-icon--ok" aria-hidden="true" />;
    case "warning":
      return <AlertTriangle size={size} className="chk-icon chk-icon--warning" aria-hidden="true" />;
    case "error":
      return <XCircle size={size} className="chk-icon chk-icon--error" aria-hidden="true" />;
    default:
      return <CircleSlash size={size} className="chk-icon" aria-hidden="true" />;
  }
}

/** Renders `system.checks` results. */
export function SystemChecksList({ checks }: { checks: readonly SystemCheck[] }) {
  return (
    <ul className="chk-list">
      {checks.map((c) => (
        <li key={c.id} className="chk-item">
          <CheckIcon status={c.status} />
          <div className="chk-item__text">
            <div className="chk-item__title">
              {c.title}
              <Badge tone={CHECK_TONE[c.status]}>{LABEL[c.status]}</Badge>
            </div>
            <div className="chk-item__detail">{c.detail}</div>
            {c.hint && <div className="chk-item__hint">{c.hint}</div>}
          </div>
        </li>
      ))}
    </ul>
  );
}
