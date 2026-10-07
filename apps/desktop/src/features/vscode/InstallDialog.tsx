/**
 * Consent dialog before downloading the VS Code server: shows what is
 * downloaded (version, size, platform, source, license, extension registry)
 * and the security consequence (extensions run as the OS user). The core
 * refuses `vscode.install` without `accept: true`.
 */
import { Button, Spinner } from "@orkestr/ui";
import { Download, ShieldAlert } from "lucide-react";
import { useEffect, useState } from "react";

import { ActionDialog } from "../files/Dialog";
import { useFormat, useT } from "../../i18n";
import { useVsCodeStore } from "./store";

export function InstallDialog({ onClose }: { onClose: () => void }) {
  const { t } = useT("vscode");
  const fmt = useFormat();
  const release = useVsCodeStore((s) => s.release);
  const releaseError = useVsCodeStore((s) => s.releaseError);
  const checking = useVsCodeStore((s) => s.checkingRelease);
  const status = useVsCodeStore((s) => s.status);
  const busy = useVsCodeStore((s) => s.busy === "install");
  const [accepted, setAccepted] = useState(false);

  useEffect(() => {
    void useVsCodeStore.getState().checkRelease();
  }, []);

  const confirm = async () => {
    if (!release || !accepted) return;
    await useVsCodeStore.getState().install(release.version);
    onClose();
  };

  const target = status?.target ?? "—";
  const rows: Array<[string, string]> = release
    ? [
        [t("consent.version"), release.version],
        [t("consent.size"), fmt.bytes(release.sizeBytes)],
        [t("consent.target"), target],
        [t("consent.source"), release.releasePage],
        [t("consent.license"), release.license],
        [t("consent.registry"), release.extensionRegistry],
      ]
    : [];

  return (
    <ActionDialog
      title={t("consent.title")}
      subtitle={t("consent.subtitle")}
      icon={<Download size={18} />}
      tone="warning"
      onCancel={onClose}
      actions={[
        { id: "cancel", label: t("consent.cancel"), variant: "ghost", autoFocus: true, onSelect: onClose },
        {
          id: "install",
          label: t("consent.confirm"),
          variant: "primary",
          icon: <Download size={14} />,
          busy,
          disabled: !release || !accepted || busy,
          onSelect: () => void confirm(),
        },
      ]}
    >
      {checking && (
        <p className="vsc-row">
          <Spinner size={14} /> {t("intro.checking")}
        </p>
      )}
      {releaseError && (
        <div className="vsc-alert" role="alert">
          <span>{t("consent.loadError", { error: releaseError })}</span>
          <Button size="sm" variant="secondary" onClick={() => void useVsCodeStore.getState().checkRelease()}>
            {t("consent.retry")}
          </Button>
        </div>
      )}
      {release && (
        <>
          <dl className="vsc-facts">
            {rows.map(([k, v]) => (
              <div key={k} className="vsc-facts__row">
                <dt>{k}</dt>
                <dd>{v}</dd>
              </div>
            ))}
          </dl>
          <p className="vsc-muted">{t("consent.checksum")}</p>
          <div className="vsc-risk" role="note">
            <ShieldAlert size={16} aria-hidden="true" />
            <div>
              <strong>{t("consent.riskTitle")}</strong>
              <p>{t("consent.risk")}</p>
              <p>{t("consent.server")}</p>
            </div>
          </div>
          <label className="vsc-check">
            <input type="checkbox" checked={accepted} onChange={(e) => setAccepted(e.target.checked)} />
            <span>{t("consent.accept")}</span>
          </label>
        </>
      )}
    </ActionDialog>
  );
}
