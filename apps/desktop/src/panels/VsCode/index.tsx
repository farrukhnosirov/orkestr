/**
 * VS Code panel: installs (with consent), starts and controls the embedded
 * VS Code server (VSCodium REH web) and opens its UI in the VS Code window
 * on the active project's folder. States: not installed → installing →
 * stopped → starting → running (window + toolbar) / error (log + retry).
 *
 * All data flows through the core (`vscode.*`); the window itself is
 * managed by the shell (see `features/vscode/windowControls.ts`).
 */
import type { VsCodeProgress, VsCodeStatus } from "@orkestr/protocol";
import { Badge, Button, EmptyState, IconButton, Spinner, type Tone } from "@orkestr/ui";
import { AlertTriangle, Code2, ExternalLink, FolderOpen, Play, RefreshCw, ScrollText, Square } from "lucide-react";
import { useEffect, useState } from "react";

import { useFormat, useT } from "../../i18n";
import { useAppStore } from "../../store/app";
import { InstallDialog } from "../../features/vscode/InstallDialog";
import { useVsCodeStore } from "../../features/vscode/store";
import { vscodeWindow } from "../../features/vscode/windowControls";
import "../../features/vscode/vscode.css";
import type { PanelProps } from "../types";

const STATE_TONE: Record<VsCodeStatus["state"], Tone> = {
  notInstalled: "neutral",
  installing: "info",
  stopped: "neutral",
  starting: "info",
  running: "success",
  restarting: "warning",
  stopping: "info",
  error: "danger",
};

function ProgressView({ progress }: { progress: VsCodeProgress | null }) {
  const { t } = useT("vscode");
  const fmt = useFormat();
  const phase = progress?.phase ?? "resolving";
  const total = progress?.total ?? null;
  const done = progress?.done ?? 0;
  const ratio = phase === "downloading" && total ? Math.min(1, done / total) : null;
  const text =
    phase === "downloading"
      ? total
        ? t("progress.downloading", { done: fmt.bytes(done), total: fmt.bytes(total) })
        : t("progress.downloadingUnknown", { done: fmt.bytes(done) })
      : t(`progress.${phase}`);
  return (
    <div className="vsc-progress">
      <div
        className="vsc-progress__bar"
        role="progressbar"
        aria-label={text}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={ratio === null ? undefined : Math.round(ratio * 100)}
      >
        <span
          className={ratio === null ? "vsc-progress__fill vsc-progress__fill--busy" : "vsc-progress__fill"}
          style={ratio === null ? undefined : { width: `${(ratio * 100).toFixed(1)}%` }}
        />
      </div>
      <div className="vsc-row vsc-muted">
        <span>{text}</span>
        {ratio !== null && <span>{fmt.percent(ratio)}</span>}
      </div>
      <Button size="sm" variant="ghost" onClick={() => void useVsCodeStore.getState().cancelInstall()}>
        {t("progress.cancel")}
      </Button>
    </div>
  );
}

function LogView() {
  const { t } = useT("vscode");
  const logs = useVsCodeStore((s) => s.logs);
  useEffect(() => {
    void useVsCodeStore.getState().loadLogs();
  }, []);
  return (
    <section className="vsc-logs" aria-label={t("failure.logs")}>
      <h4>{t("failure.logs")}</h4>
      <pre>{logs.length > 0 ? logs.slice(-200).join("\n") : t("failure.noLogs")}</pre>
    </section>
  );
}

export default function VsCodePanel({ projectId }: PanelProps) {
  const { t } = useT("vscode");
  const status = useVsCodeStore((s) => s.status);
  const progress = useVsCodeStore((s) => s.progress);
  const error = useVsCodeStore((s) => s.error);
  const busy = useVsCodeStore((s) => s.busy);
  const windowOpen = useVsCodeStore((s) => s.windowOpen);
  const lastUrl = useVsCodeStore((s) => s.lastUrl);
  const connected = useAppStore((s) => s.connection.state === "open");
  const project = useAppStore((s) => s.projects.find((p) => p.id === projectId) ?? null);
  const [installOpen, setInstallOpen] = useState(false);
  const [showLogs, setShowLogs] = useState(false);

  useEffect(() => useVsCodeStore.getState().attach(), []);
  useEffect(() => {
    if (connected) void useVsCodeStore.getState().load();
  }, [connected]);

  const folderProjectId = project?.rootPath ? project.id : null;
  const title = t("run.windowTitle", { name: project?.name ?? t("panel.label") });
  const open = () => void useVsCodeStore.getState().open(folderProjectId, title);
  const stop = () => void useVsCodeStore.getState().stop();

  if (!status) {
    return (
      <div className="vsc-panel vsc-panel--center">
        <Spinner size={18} /> <span>{t("panel.loading")}</span>
      </div>
    );
  }
  if (!status.target) {
    return (
      <div className="vsc-panel vsc-panel--center">
        <EmptyState
          icon={<AlertTriangle size={28} />}
          title={t("panel.label")}
          description={t("panel.unsupported", { reason: status.unsupportedReason ?? "" })}
        />
      </div>
    );
  }

  const state = status.state;
  const running = state === "running";
  const folderLine = !project
    ? t("panel.noProject")
    : project.rootPath
      ? t("panel.project", { path: project.rootPath })
      : t("panel.projectNoFolder");

  return (
    <div className="vsc-panel">
      <header className="vsc-head">
        <Code2 size={16} aria-hidden="true" />
        <h3>{t("panel.label")}</h3>
        <Badge tone={STATE_TONE[state]}>{t(`state.${state}`)}</Badge>
        {status.installed && <span className="vsc-muted">{status.installed.version}</span>}
        <span className="vsc-spacer" />
        {running && (
          <div className="vsc-toolbar">
            <IconButton
              label={t("run.reload")}
              icon={<RefreshCw size={14} />}
              tooltip="bottom"
              disabled={!windowOpen}
              onClick={() => void vscodeWindow.reload()}
            />
            <IconButton
              label={t("run.external")}
              icon={<ExternalLink size={14} />}
              tooltip="bottom"
              disabled={!lastUrl}
              onClick={() => lastUrl && void vscodeWindow.openExternal(lastUrl)}
            />
            <IconButton
              label={t("run.stop")}
              icon={<Square size={14} />}
              tooltip="bottom"
              variant="danger"
              disabled={busy === "stop"}
              onClick={stop}
            />
          </div>
        )}
      </header>

      <div className="vsc-body">
        {error && (
          <div className="vsc-alert" role="alert">
            <AlertTriangle size={14} aria-hidden="true" />
            <span>{error}</span>
          </div>
        )}

        {state === "notInstalled" && (
          <section className="vsc-card">
            <h4>{t("intro.title")}</h4>
            <p>{t("intro.body")}</p>
            <p className="vsc-muted">{t("settings.legal")}</p>
            <Button variant="primary" leftIcon={<Code2 size={14} />} onClick={() => setInstallOpen(true)}>
              {t("intro.install")}
            </Button>
          </section>
        )}

        {state === "installing" && (
          <section className="vsc-card">
            <h4>{t("state.installing")}</h4>
            <ProgressView progress={progress} />
          </section>
        )}

        {(state === "stopped" || running) && (
          <section className="vsc-card">
            <p className="vsc-row">
              <FolderOpen size={14} aria-hidden="true" />
              <span className="vsc-path" title={project?.rootPath ?? undefined}>
                {folderLine}
              </span>
            </p>
            {running && (
              <>
                <p className="vsc-muted">{windowOpen ? t("run.windowOpen") : t("run.windowClosed")}</p>
                {status.running && (
                  <p className="vsc-muted">
                    {t("run.port", { port: status.running.port })}
                    {status.running.restarts > 0 && ` · ${t("run.restarts", { count: status.running.restarts })}`}
                  </p>
                )}
              </>
            )}
            <div className="vsc-actions">
              <Button variant="primary" leftIcon={<Play size={14} />} loading={busy === "start"} onClick={open}>
                {running && windowOpen ? t("run.focus") : t("run.open")}
              </Button>
            </div>
            <ul className="vsc-hints">
              <li>{t("run.why")}</li>
              <li>{t("run.extensionsHint")}</li>
              <li>{t("run.syncHint")}</li>
            </ul>
          </section>
        )}

        {(state === "starting" || state === "restarting" || state === "stopping") && (
          <section className="vsc-card vsc-row">
            <Spinner size={16} />
            <span>
              {state === "starting" ? t("run.starting") : state === "restarting" ? t("run.restarting") : t("run.stopping")}
            </span>
          </section>
        )}

        {state === "error" && (
          <section className="vsc-card">
            <h4>{t("failure.title")}</h4>
            {status.error && <p className="vsc-error">{status.error}</p>}
            <div className="vsc-actions">
              {status.installed ? (
                <Button variant="primary" leftIcon={<RefreshCw size={14} />} loading={busy === "start"} onClick={open}>
                  {t("failure.retry")}
                </Button>
              ) : (
                <Button variant="primary" leftIcon={<Code2 size={14} />} onClick={() => setInstallOpen(true)}>
                  {t("intro.install")}
                </Button>
              )}
              <Button variant="ghost" leftIcon={<ScrollText size={14} />} onClick={() => setShowLogs((v) => !v)}>
                {showLogs ? t("failure.hideLogs") : t("failure.showLogs")}
              </Button>
            </div>
            {showLogs && <LogView />}
          </section>
        )}

        {state !== "error" && (running || state === "stopped") && (
          <div className="vsc-actions">
            <Button size="sm" variant="ghost" leftIcon={<ScrollText size={14} />} onClick={() => setShowLogs((v) => !v)}>
              {showLogs ? t("failure.hideLogs") : t("failure.showLogs")}
            </Button>
          </div>
        )}
        {state !== "error" && showLogs && <LogView />}
      </div>

      {installOpen && <InstallDialog onClose={() => setInstallOpen(false)} />}
    </div>
  );
}
