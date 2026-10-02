import type { CheckStatus, SystemCheck } from "@orkestr/protocol";
import { Badge, type Tone } from "@orkestr/ui";
import { CheckCircle2, CircleSlash, XCircle, AlertTriangle } from "lucide-react";

import { useT } from "../../../i18n";

import { checkTexts } from "./checkText";

export const CHECK_TONE: Record<CheckStatus, Tone> = {
  ok: "success",
  warning: "warning",
  error: "danger",
  skipped: "neutral",
};

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
  const { t } = useT("settings");
  return (
    <ul className="chk-list">
      {checks.map((c) => {
        const text = checkTexts(c);
        return (
        <li key={c.id} className="chk-item">
          <CheckIcon status={c.status} />
          <div className="chk-item__text">
            <div className="chk-item__title">
              {text.title}
              <Badge tone={CHECK_TONE[c.status]}>{t(`checkStatus.${c.status}`)}</Badge>
            </div>
            <div className="chk-item__detail">{text.detail}</div>
            {text.hint && <div className="chk-item__hint">{text.hint}</div>}
          </div>
        </li>
        );
      })}
    </ul>
  );
}
