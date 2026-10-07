/** Settings → VS Code: version, updates, data folder, auto-stop, removal. */
import { Badge, Button, Spinner } from "@orkestr/ui";
import { ShieldAlert, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";

import { ActionDialog } from "../files/Dialog";
import { useFormat, useT } from "../../i18n";
import { compareVersions } from "./logic";
import { SettingGroup, SettingRow, Switch } from "../settings/controls";
import type { SettingsSectionProps } from "../settings/sections";
import { InstallDialog } from "./InstallDialog";
import { useVsCodeStore } from "./store";
import "./vscode.css";

const IDLE_MINUTES = [1, 2, 5, 15, 30, 60] as const;

export function VsCodeSettingsSection(_props: SettingsSectionProps) {
  const { t } = useT("vscode");
  const fmt = useFormat();
  const status = useVsCodeStore((s) => s.status);
  const release = useVsCodeStore((s) => s.release);
  const releaseError = useVsCodeStore((s) => s.releaseError);
  const checking = useVsCodeStore((s) => s.checkingRelease);
  const busy = useVsCodeStore((s) => s.busy);
  const error = useVsCodeStore((s) => s.error);
  const [installOpen, setInstallOpen] = useState(false);
  const [removeOpen, setRemoveOpen] = useState(false);
  const [purge, setPurge] = useState(false);

  useEffect(() => {
    void useVsCodeStore.getState().load();
  }, []);

  if (!status) {
    return (
      <div className="set-section">
        <div className="set-loading">
          <Spinner size={16} /> {t("panel.loading")}
        </div>
      </div>
    );
  }

  const installed = status.installed;
  const config = status.config;
  const running = status.state === "running" || status.state === "starting" || status.state === "restarting";
  const newer = release && installed ? compareVersions(release.version, installed.version) > 0 : false;
  const idleMinutes = Math.round(config.idleStopSecs / 60);
  const idleOptions = IDLE_MINUTES.includes(idleMinutes as (typeof IDLE_MINUTES)[number])
    ? IDLE_MINUTES
    : [...IDLE_MINUTES, idleMinutes].sort((a, b) => a - b);

  return (
    <div className="set-section">
      {error && (
        <div className="vsc-alert" role="alert">
          {error}
        </div>
      )}
      <SettingGroup title={t("settings.status")}>
        <SettingRow title={t("settings.version")} description={status.target ? t("settings.targetValue", { target: status.target }) : status.unsupportedReason}>
          <span className="vsc-row">
            <Badge tone={running ? "success" : status.state === "error" ? "danger" : "neutral"}>{t(`state.${status.state}`)}</Badge>
            <span>{installed ? installed.version : t("settings.notInstalled")}</span>
          </span>
        </SettingRow>
        <SettingRow title={t("settings.dataDir")} description={t("settings.dataDirHint")} stacked>
          <code className="vsc-path">{status.dataDir}</code>
        </SettingRow>
      </SettingGroup>

      <SettingGroup title={t("settings.updates")}>
        <SettingRow
          title={t("settings.checkUpdate")}
          description={
            checking
              ? t("intro.checking")
              : releaseError
                ? releaseError
                : release
                  ? newer || !installed
                    ? t("settings.updateAvailable", { version: release.version, size: fmt.bytes(release.sizeBytes) })
                    : t("settings.upToDate", { version: release.version })
                  : undefined
          }
        >
          <span className="vsc-actions">
            <Button size="sm" variant="secondary" loading={checking} disabled={!status.target} onClick={() => void useVsCodeStore.getState().checkRelease()}>
              {t("settings.checkUpdate")}
            </Button>
            {(newer || !installed) && status.target && (
              <Button size="sm" variant="primary" disabled={running || status.state === "installing"} onClick={() => setInstallOpen(true)}>
                {installed ? t("settings.update") : t("settings.install")}
              </Button>
            )}
          </span>
        </SettingRow>
      </SettingGroup>

      <SettingGroup title={t("settings.lifecycle")}>
        <SettingRow title={t("settings.autoStop")} description={t("settings.autoStopDesc")}>
          <Switch
            checked={config.autoStop}
            label={t("settings.autoStop")}
            busy={busy === "config"}
            onChange={(v) => void useVsCodeStore.getState().setConfig({ autoStop: v })}
          />
        </SettingRow>
        <SettingRow title={t("settings.idleStop")}>
          <select
            className="vsc-select"
            aria-label={t("settings.idleStop")}
            value={idleMinutes}
            disabled={!config.autoStop}
            onChange={(e) => void useVsCodeStore.getState().setConfig({ idleStopSecs: Number(e.target.value) * 60 })}
          >
            {idleOptions.map((m) => (
              <option key={m} value={m}>
                {t("settings.minutes", { count: m })}
              </option>
            ))}
          </select>
        </SettingRow>
        <SettingRow title={t("settings.autoRestart")} description={t("settings.autoRestartDesc")}>
          <Switch
            checked={config.autoRestart}
            label={t("settings.autoRestart")}
            busy={busy === "config"}
            onChange={(v) => void useVsCodeStore.getState().setConfig({ autoRestart: v })}
          />
        </SettingRow>
        {running && (
          <SettingRow title={t("settings.stopNow")}>
            <Button size="sm" variant="secondary" loading={busy === "stop"} onClick={() => void useVsCodeStore.getState().stop()}>
              {t("settings.stopNow")}
            </Button>
          </SettingRow>
        )}
      </SettingGroup>

      <SettingGroup title={t("settings.security")}>
        <div className="vsc-risk" role="note">
          <ShieldAlert size={16} aria-hidden="true" />
          <div>
            <p>{t("settings.securityBody")}</p>
            <p>{t("settings.legal")}</p>
          </div>
        </div>
      </SettingGroup>

      {installed && (
        <SettingGroup title={t("settings.remove")}>
          <SettingRow title={t("settings.uninstall")} description={t("settings.uninstallBody")}>
            <Button size="sm" variant="danger" leftIcon={<Trash2 size={14} />} disabled={status.state === "installing"} onClick={() => setRemoveOpen(true)}>
              {t("settings.uninstall")}
            </Button>
          </SettingRow>
        </SettingGroup>
      )}

      {installOpen && <InstallDialog onClose={() => setInstallOpen(false)} />}
      {removeOpen && (
        <ActionDialog
          title={t("settings.uninstallTitle")}
          subtitle={t("settings.uninstallBody")}
          icon={<Trash2 size={18} />}
          tone="danger"
          onCancel={() => setRemoveOpen(false)}
          actions={[
            { id: "cancel", label: t("consent.cancel"), variant: "ghost", autoFocus: true, onSelect: () => setRemoveOpen(false) },
            {
              id: "remove",
              label: t("settings.uninstallConfirm"),
              variant: "danger",
              busy: busy === "uninstall",
              onSelect: () => {
                void useVsCodeStore
                  .getState()
                  .uninstall(purge)
                  .then(() => setRemoveOpen(false));
              },
            },
          ]}
        >
          <label className="vsc-check">
            <input type="checkbox" checked={purge} onChange={(e) => setPurge(e.target.checked)} />
            <span>{t("settings.purge")}</span>
          </label>
        </ActionDialog>
      )}
    </div>
  );
}
