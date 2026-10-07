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
import { Badge, Button, EmptyState, IconButton, Input, Spinner, type Tone } from "@orkestr/ui";
import {
  AlertTriangle,
  Blocks,
  Code2,
  ExternalLink,
  FolderOpen,
  PackagePlus,
  Play,
  RefreshCw,
  ScrollText,
  Square,
} from "lucide-react";
import { type FormEvent, useEffect, useRef, useState } from "react";

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

/** Suggested Open VSX extensions (one click). */
const SUGGESTED: readonly { id: string; labelKey: "extensions.claude" }[] = [
  { id: "anthropic.claude-code", labelKey: "extensions.claude" },
];

/**
 * "Extensions" section: Open VSX extensions of Orkestr's VS Code (list +
 * install by id). Brought into view by Ctrl+Shift+X / "Install extension…".
 */
function ExtensionsSection({ installed, windowOpen }: { installed: boolean; windowOpen: boolean }) {
  const { t } = useT("vscode");
  const extensions = useVsCodeStore((s) => s.extensions);
  const error = useVsCodeStore((s) => s.extensionsError);
  const installing = useVsCodeStore((s) => s.installingExtension);
  const view = useVsCodeStore((s) => s.view);
  const nonce = useVsCodeStore((s) => s.viewNonce);
  const [id, setId] = useState("");
  const [justInstalled, setJustInstalled] = useState(false);
  const sectionRef = useRef<HTMLElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (installed) void useVsCodeStore.getState().loadExtensions();
  }, [installed]);
  useEffect(() => {
    if (view !== "extensions") return;
    sectionRef.current?.scrollIntoView?.({ block: "start" });
    inputRef.current?.focus();
  }, [view, nonce]);

  const install = async (value: string) => {
    if (await useVsCodeStore.getState().installExtension(value)) {
      setId("");
      setJustInstalled(true);
    }
  };
  const submit = (e: FormEvent) => {
    e.preventDefault();
    void install(id);
  };
  const has = (ext: string) => extensions?.some((x) => x.id === ext) ?? false;

  return (
    <section className="vsc-card" ref={sectionRef} aria-labelledby="vsc-extensions-title">
      <h4 id="vsc-extensions-title" className="vsc-row">
        <Blocks size={14} aria-hidden="true" />
        {t("extensions.title")}
      </h4>
      <p className="vsc-muted">{t("extensions.subtitle")}</p>
      {!installed ? (
        <p className="vsc-muted">{t("extensions.notInstalled")}</p>
      ) : (
        <>
          <form className="vsc-row" onSubmit={submit}>
            <Input
              ref={inputRef}
              aria-label={t("extensions.idLabel")}
              placeholder={t("extensions.idPlaceholder")}
              value={id}
              maxLength={200}
              spellCheck={false}
              autoComplete="off"
              onChange={(e) => setId(e.target.value)}
              disabled={installing !== null}
            />
            <Button
              type="submit"
              variant="primary"
              leftIcon={<PackagePlus size={14} />}
              loading={installing !== null}
              disabled={!id.trim()}
            >
              {t("extensions.install")}
            </Button>
          </form>
          {installing && (
            <p className="vsc-row vsc-muted" role="status">
              <Spinner size={14} /> {t("extensions.installing", { id: installing })}
            </p>
          )}
          {error && (
            <div className="vsc-alert" role="alert">
              <AlertTriangle size={14} aria-hidden="true" />
              <span>{error}</span>
            </div>
          )}
          {SUGGESTED.some((x) => !has(x.id)) && (
            <div className="vsc-row">
              <span className="vsc-muted">{t("extensions.suggested")}:</span>
              {SUGGESTED.filter((x) => !has(x.id)).map((x) => (
                <Button
                  key={x.id}
                  size="sm"
                  variant="ghost"
                  leftIcon={<PackagePlus size={14} />}
                  disabled={installing !== null}
                  onClick={() => void install(x.id)}
                >
                  {t(x.labelKey)}
                </Button>
              ))}
            </div>
          )}
          <div className="vsc-row">
            <strong>{t("extensions.installed")}</strong>
            <span className="vsc-spacer" />
            <IconButton
              label={t("extensions.refresh")}
              icon={<RefreshCw size={14} />}
              tooltip="bottom"
              onClick={() => void useVsCodeStore.getState().loadExtensions()}
            />
          </div>
          {extensions === null ? (
            <Spinner size={14} />
          ) : extensions.length === 0 ? (
            <p className="vsc-muted">{t("extensions.none")}</p>
          ) : (
            <ul className="vsc-extensions">
              {extensions.map((x) => (
                <li key={x.id}>
                  <code>{x.id}</code>
                  {x.version && <span className="vsc-muted">{x.version}</span>}
                </li>
              ))}
            </ul>
          )}
          <ul className="vsc-hints">
            {justInstalled && windowOpen && <li>{t("extensions.reloadHint")}</li>}
            <li>{t("extensions.browseHint")}</li>
          </ul>
        </>
      )}
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

        {state !== "installing" && <ExtensionsSection installed={status.installed !== null} windowOpen={windowOpen} />}

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
