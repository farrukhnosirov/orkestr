/**
 * Marketplace dialogs: publisher key trust (fingerprint confirmation, TOFU)
 * and the plugin permission prompt (pending grants after install).
 */
import { Badge, Button } from "@orkestr/ui";
import { KeyRound, ShieldCheck, X } from "lucide-react";
import { type ReactNode, useState } from "react";

import { Overlay } from "../../components/Overlay";
import { useT } from "../../i18n";
import type { GrantDecision, PermissionRequest, PublisherKey } from "./api";
import { fingerprintRows, initialDecisions } from "./model";

function DialogHeader({
  id,
  icon,
  title,
  subtitle,
  onClose,
}: {
  id: string;
  icon: ReactNode;
  title: string;
  subtitle: string;
  onClose: () => void;
}) {
  const { t } = useT(["marketplace", "common"]);
  return (
    <header className="dialog__header">
      <div className="dialog__icon">{icon}</div>
      <div>
        <h2 id={id} className="dialog__title">
          {title}
        </h2>
        <p className="dialog__subtitle">{subtitle}</p>
      </div>
      <button
        type="button"
        className="dialog__close"
        aria-label={t("common:actions.close")}
        onClick={onClose}
      >
        <X size={16} />
      </button>
    </header>
  );
}

/** Shows the fingerprint; trusting pins the key for this and later installs. */
export function TrustKeyDialog({
  pkey,
  busy,
  onTrust,
  onCancel,
}: {
  pkey: PublisherKey;
  busy: boolean;
  onTrust: () => void;
  onCancel: () => void;
}) {
  const { t } = useT(["marketplace", "common"]);
  return (
    <Overlay onDismiss={onCancel}>
      <div
        className="dialog mp-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="mp-trust-title"
        onKeyDown={(e) => e.key === "Escape" && onCancel()}
      >
        <DialogHeader
          id="mp-trust-title"
          icon={<KeyRound size={18} />}
          title={t("trust.title")}
          subtitle={t("trust.subtitle", { publisher: pkey.publisher })}
          onClose={onCancel}
        />
        <div className="dialog__body">
          <p className="mp-muted">{t("trust.explain")}</p>
          <div className="mp-fingerprint" aria-label={t("keys.fingerprint")}>
            {fingerprintRows(pkey.fingerprint).map((row) => (
              <code key={row}>{row}</code>
            ))}
          </div>
          <p className="mp-muted">{t("trust.keyId", { keyId: pkey.keyId })}</p>
        </div>
        <footer className="dialog__footer">
          <Button variant="ghost" onClick={onCancel}>
            {t("common:actions.cancel")}
          </Button>
          <Button variant="primary" loading={busy} onClick={onTrust}>
            {t("trust.confirm")}
          </Button>
        </footer>
      </div>
    </Overlay>
  );
}

/** Permission prompt for an installed plugin / widget package (`plugins.grant`). */
export function PermissionDialog({
  name,
  pending,
  busy,
  onDecide,
  onCancel,
}: {
  name: string;
  pending: PermissionRequest[];
  busy: boolean;
  onDecide: (decisions: GrantDecision[]) => void;
  onCancel: () => void;
}) {
  const { t } = useT(["marketplace", "security", "common"]);
  const [decisions, setDecisions] = useState<GrantDecision[]>(() =>
    initialDecisions(pending),
  );
  const toggle = (i: number) =>
    setDecisions((ds) =>
      ds.map((d, j) => (j === i ? { ...d, allow: !d.allow } : d)),
    );
  return (
    <Overlay onDismiss={onCancel}>
      <div
        className="dialog mp-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="mp-perm-title"
        onKeyDown={(e) => e.key === "Escape" && onCancel()}
      >
        <DialogHeader
          id="mp-perm-title"
          icon={<ShieldCheck size={18} />}
          title={t("permissions.title")}
          subtitle={t("permissions.subtitle", { name })}
          onClose={onCancel}
        />
        <div className="dialog__body">
          <ul className="mp-perms" aria-label={t("permissions.title")}>
            {pending.map((p, i) => (
              <li key={`${p.capability}:${p.scope ?? ""}`}>
                <label className="mp-perm">
                  <input
                    type="checkbox"
                    checked={decisions[i]?.allow ?? false}
                    onChange={() => toggle(i)}
                  />
                  <span>
                    <strong>{t(`security:capability.${p.capability}`)}</strong>
                    {p.scope && <Badge tone="neutral">{p.scope}</Badge>}
                    <span className="mp-muted mp-perm__reason">{p.reason}</span>
                  </span>
                </label>
              </li>
            ))}
          </ul>
          <p className="mp-muted">{t("permissions.hint")}</p>
        </div>
        <footer className="dialog__footer">
          <Button variant="ghost" onClick={onCancel}>
            {t("permissions.later")}
          </Button>
          <Button
            variant="primary"
            loading={busy}
            onClick={() => onDecide(decisions)}
          >
            {t("permissions.apply")}
          </Button>
        </footer>
      </div>
    </Overlay>
  );
}
