/**
 * VS Code panel (ADR-0043): installs (with consent), starts and shows the
 * VS Code workbench (VSCodium REH web — every installed extension with the
 * real extension host) **inside the Orkestr window** on the active project's
 * folder, or, as an option, in the separate VS Code window (ADR-0025).
 * Tabs: Workbench · Extensions (management) · Commands (launcher) · Log.
 *
 * Lifecycle: the server starts when the panel opens (heartbeat while it is
 * mounted, idle auto-stop after it closes), the frame reloads after a crash
 * restart, and the Orkestr theme is pushed into the workbench through the
 * bridge extension. All data flows through the core (`vscode.*`); the frame
 * URL carries the connection token and lives in memory only.
 */
import type { VsCodeProgress, VsCodeStatus } from "@orkestr/protocol";
import { Badge, Button, EmptyState, IconButton, Spinner, Tabs, type Tone } from "@orkestr/ui";
import { AlertTriangle, AppWindow, Code2, ExternalLink, FolderOpen, Play, RefreshCw, Square } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";

import { useFormat, useT } from "../../i18n";
import { useAppStore } from "../../store/app";
import { usePaletteStore } from "../../features/search/paletteStore";
import { useThemeRevision } from "../../features/theme/useThemeRevision";
import { EMBED_ALLOW, EMBED_SANDBOX } from "../../features/vscode/embed";
import { ExtensionLauncher } from "../../features/vscode/ExtensionLauncher";
import { ExtensionsManager } from "../../features/vscode/ExtensionsManager";
import { InstallDialog } from "../../features/vscode/InstallDialog";
import { embedsVsCode } from "../../features/vscode/quickAccess";
import { useVsCodeStore, VSCODE_FOCUS_OUT_EVENT, VSCODE_PALETTE_EVENT } from "../../features/vscode/store";
import { vscodeWindow } from "../../features/vscode/windowControls";
import "../../features/vscode/vscode.css";
import type { PanelProps } from "../types";

type Tab = "workbench" | "extensions" | "commands" | "log";

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

/** The workbench frame (kept mounted while the other tabs are shown). */
function Workbench({
  url,
  reloadKey,
  title,
  onFocusOut,
}: {
  url: string;
  reloadKey: number;
  title: string;
  onFocusOut: () => void;
}) {
  const { t } = useT("vscode");
  const frame = useRef<HTMLIFrameElement>(null);
  const [loaded, setLoaded] = useState(false);
  useEffect(() => setLoaded(false), [url, reloadKey]);
  useEffect(() => {
    // Ctrl+Alt+O / Ctrl+Alt+K inside VS Code (bridge keybindings) give the keyboard back.
    const out = () => {
      if (document.activeElement === frame.current) onFocusOut();
    };
    const palette = () => {
      if (document.activeElement === frame.current) onFocusOut();
      usePaletteStore.getState().openPalette("");
    };
    window.addEventListener(VSCODE_FOCUS_OUT_EVENT, out);
    window.addEventListener(VSCODE_PALETTE_EVENT, palette);
    return () => {
      window.removeEventListener(VSCODE_FOCUS_OUT_EVENT, out);
      window.removeEventListener(VSCODE_PALETTE_EVENT, palette);
    };
  }, [onFocusOut]);
  return (
    <div className="vsc-embed">
      <iframe
        key={reloadKey}
        ref={frame}
        className="vsc-embed__frame"
        title={title}
        src={url}
        sandbox={EMBED_SANDBOX}
        allow={EMBED_ALLOW}
        referrerPolicy="no-referrer"
        onLoad={() => setLoaded(true)}
        data-testid="vscode-embed"
      />
      {!loaded && (
        <div className="vsc-embed__loading" role="status">
          <Spinner size={18} /> <span>{t("embed.loading")}</span>
        </div>
      )}
    </div>
  );
}

function LogTab() {
  const { t } = useT("vscode");
  const running = useVsCodeStore((s) => s.status?.running ?? null);
  return (
    <>
      {running && (
        <p className="vsc-muted">
          {t("run.port", { port: running.port })}
          {running.restarts > 0 && ` · ${t("run.restarts", { count: running.restarts })}`}
        </p>
      )}
      <LogView />
    </>
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
  const embedUrl = useVsCodeStore((s) => s.embedUrl);
  const embedKey = useVsCodeStore((s) => s.embedKey);
  const embedProjectId = useVsCodeStore((s) => s.embedProjectId);
  const embedUnsupported = useVsCodeStore((s) => s.embedUnsupported);
  const view = useVsCodeStore((s) => s.view);
  const nonce = useVsCodeStore((s) => s.viewNonce);
  const connected = useAppStore((s) => s.connection.state === "open");
  const project = useAppStore((s) => s.projects.find((p) => p.id === projectId) ?? null);
  const themeRev = useThemeRevision();
  const [installOpen, setInstallOpen] = useState(false);
  const [tab, setTab] = useState<Tab>(view === "extensions" ? "extensions" : "workbench");
  /** Project the panel last auto-started VS Code for (`undefined` = none yet). */
  const autoStarted = useRef<string | null | undefined>(undefined);
  const lastStart = useRef<string | null>(null);
  const headRef = useRef<HTMLElement>(null);

  useEffect(() => useVsCodeStore.getState().attach(), []);
  useEffect(() => {
    if (connected) void useVsCodeStore.getState().load();
  }, [connected]);
  useEffect(() => {
    if (nonce > 0) setTab(view === "extensions" ? "extensions" : "workbench");
  }, [view, nonce]);

  const state = status?.state ?? "stopped";
  const running = state === "running";
  const embed = status !== null && embedsVsCode();
  const hasInstall = status?.installed != null;
  const folderProjectId = project?.rootPath ? project.id : null;
  const title = t("run.windowTitle", { name: project?.name ?? t("panel.label") });
  const startedAt = status?.running?.startedAt ?? null;

  // Start when the panel opens and follow the active project — at most once
  // per project and mount (a user Stop or a failed start is not retried).
  useEffect(() => {
    if (!embed || !hasInstall || busy || (state !== "stopped" && state !== "running")) return;
    if (autoStarted.current === folderProjectId) return;
    autoStarted.current = folderProjectId;
    const store = useVsCodeStore.getState();
    if (running && store.embedUrl !== null && store.embedProjectId === folderProjectId) return;
    void store.startEmbedded(folderProjectId);
  }, [embed, hasInstall, state, running, folderProjectId, busy]);

  // A crash restart is a new server process: reload the frame.
  useEffect(() => {
    if (!startedAt) return;
    const prev = lastStart.current;
    lastStart.current = startedAt;
    if (prev && prev !== startedAt && embed && useVsCodeStore.getState().embedUrl) {
      void useVsCodeStore.getState().startEmbedded(embedProjectId, true);
    }
  }, [startedAt, embed, embedProjectId]);

  // Orkestr theme → workbench colors (when the bridge connects and on every theme change).
  const bridge = status?.bridgeConnected === true;
  const syncTheme = status?.config.syncTheme ?? true;
  useEffect(() => {
    if (!bridge || !syncTheme) return;
    const id = setTimeout(() => void useVsCodeStore.getState().syncTheme(), 250);
    return () => clearTimeout(id);
  }, [bridge, syncTheme, themeRev]);

  const focusOut = useCallback(() => {
    headRef.current?.querySelector<HTMLElement>("[role=tab][aria-selected=true]")?.focus();
  }, []);

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

  const openWindow = () => void useVsCodeStore.getState().open(folderProjectId, title);
  const start = () => {
    if (embed) void useVsCodeStore.getState().startEmbedded(folderProjectId);
    else openWindow();
  };
  const stop = () => void useVsCodeStore.getState().stop();
  const folderLine = !project
    ? t("panel.noProject")
    : project.rootPath
      ? t("panel.project", { path: project.rootPath })
      : t("panel.projectNoFolder");
  const showFrame = embed && embedUrl !== null && (running || state === "restarting");

  const tabs = [
    { id: "workbench" as const, label: t("tabs.workbench") },
    { id: "extensions" as const, label: t("tabs.extensions") },
    { id: "commands" as const, label: t("tabs.commands") },
    { id: "log" as const, label: t("tabs.log") },
  ];

  return (
    <div className="vsc-panel">
      <header className="vsc-head" ref={headRef}>
        <Code2 size={16} aria-hidden="true" />
        <h3>{t("panel.label")}</h3>
        <Badge tone={STATE_TONE[state]}>{t(`state.${state}`)}</Badge>
        {status.installed && <span className="vsc-muted">{status.installed.version}</span>}
        <Tabs items={tabs} value={tab} onChange={setTab} aria-label={t("tabs.label")} className="vsc-tabs" />
        <span className="vsc-spacer" />
        {running && (
          <div className="vsc-toolbar">
            {embed ? (
              <IconButton
                label={t("embed.reload")}
                icon={<RefreshCw size={14} />}
                tooltip="bottom"
                disabled={!embedUrl}
                onClick={() => useVsCodeStore.getState().reloadEmbedded()}
              />
            ) : (
              <IconButton
                label={t("run.reload")}
                icon={<RefreshCw size={14} />}
                tooltip="bottom"
                disabled={!windowOpen}
                onClick={() => void vscodeWindow.reload()}
              />
            )}
            <IconButton
              label={t("embed.separateWindow")}
              icon={<AppWindow size={14} />}
              tooltip="bottom"
              disabled={busy === "start"}
              onClick={openWindow}
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

      <div className="vsc-stack">
        <div className="vsc-tabpanel" hidden={tab !== "workbench"}>
          {showFrame && embedUrl ? (
            <Workbench url={embedUrl} reloadKey={embedKey} title={title} onFocusOut={focusOut} />
          ) : (
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
                  {running && !embed && (
                    <p className="vsc-muted">{windowOpen ? t("run.windowOpen") : t("run.windowClosed")}</p>
                  )}
                  {embedUnsupported && <p className="vsc-muted">{t("embed.unsupported")}</p>}
                  <div className="vsc-actions">
                    <Button variant="primary" leftIcon={<Play size={14} />} loading={busy === "start"} onClick={start}>
                      {embed ? t("embed.start") : running && windowOpen ? t("run.focus") : t("run.open")}
                    </Button>
                    {embed && (
                      <Button variant="ghost" leftIcon={<AppWindow size={14} />} onClick={openWindow}>
                        {t("embed.separateWindow")}
                      </Button>
                    )}
                  </div>
                  <ul className="vsc-hints">
                    <li>{embed ? t("embed.why") : t("run.why")}</li>
                    {embed && <li>{t("embed.keys")}</li>}
                    <li>{t("run.syncHint")}</li>
                  </ul>
                </section>
              )}

              {(state === "starting" || state === "restarting" || state === "stopping") && (
                <section className="vsc-card vsc-row" role="status">
                  <Spinner size={16} />
                  <span>
                    {state === "starting"
                      ? t("run.starting")
                      : state === "restarting"
                        ? t("run.restarting")
                        : t("run.stopping")}
                  </span>
                </section>
              )}

              {state === "error" && (
                <section className="vsc-card">
                  <h4>{t("failure.title")}</h4>
                  {status.error && <p className="vsc-error">{status.error}</p>}
                  <div className="vsc-actions">
                    {hasInstall ? (
                      <Button variant="primary" leftIcon={<RefreshCw size={14} />} loading={busy === "start"} onClick={start}>
                        {t("failure.retry")}
                      </Button>
                    ) : (
                      <Button variant="primary" leftIcon={<Code2 size={14} />} onClick={() => setInstallOpen(true)}>
                        {t("intro.install")}
                      </Button>
                    )}
                    <Button variant="ghost" onClick={() => setTab("log")}>
                      {t("failure.showLogs")}
                    </Button>
                  </div>
                </section>
              )}
            </div>
          )}
        </div>

        {tab === "extensions" && (
          <div className="vsc-tabpanel">
            <div className="vsc-body">
              <ExtensionsManager projectId={folderProjectId} installed={hasInstall && state !== "installing"} />
            </div>
          </div>
        )}
        {tab === "commands" && (
          <div className="vsc-tabpanel">
            <div className="vsc-body">
              {hasInstall ? (
                <section className="vsc-card vsc-card--wide">
                  <ExtensionLauncher />
                </section>
              ) : (
                <p className="vsc-muted">{t("extensions.notInstalled")}</p>
              )}
            </div>
          </div>
        )}
        {tab === "log" && (
          <div className="vsc-tabpanel">
            <div className="vsc-body">
              <LogTab />
            </div>
          </div>
        )}
      </div>

      {installOpen && <InstallDialog onClose={() => setInstallOpen(false)} />}
    </div>
  );
}
