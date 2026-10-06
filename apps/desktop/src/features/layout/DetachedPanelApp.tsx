/**
 * Root of a detached panel window (§28, ADR-0013): frameless, custom title
 * bar with a re-dock button, rendering the same registry panel component.
 * It connects to the same core over RPC (its own WebSocket, same token) —
 * no data flows through the shell. Emergency Stop and approvals stay
 * reachable in every window (§70, ADR-0015).
 */
import "./layout.css";

import { EmptyState } from "@orkestr/ui";
import { Copy, Minus, PanelLeftOpen, Square, X } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { ConnectionScreen } from "../../components/ConnectionScreen";
import { TOPIC_GLOBAL } from "@orkestr/protocol";

import { bootstrapCore, getClient } from "../../core/client";
import { applyLanguagePreference, languagePreference, useT } from "../../i18n";
import { isTauri, windowControls } from "../../lib/tauri";
import { getPanel } from "../../panels/registry";
import { useAppStore } from "../../store/app";
import { Toasts } from "../notifications/NotificationCenter";
import { EmergencyStopButton } from "../security/EmergencyStop";
import { ApprovalsBadge } from "../security/ApprovalsInbox";
import { SecurityLayer } from "../security/SecurityLayer";
import { useSettingsStore } from "../settings/settingsStore";
import { useThemeEngine } from "../theme/ThemeRuntime";
import type { DetachedRoute } from "./route";
import { windowsApi } from "./windowsApi";

/** Follows settings (language, theme) like the main window, without its side effects (onboarding, tray, resume). */
function useDetachedSettings(ready: boolean) {
  useEffect(() => {
    if (!ready) return;
    const apply = () => {
      const s = useSettingsStore.getState().settings;
      if (!s) return;
      if (s.language !== languagePreference()) void applyLanguagePreference(s.language);
      const app = useAppStore.getState();
      if (s.themeId && s.themeId !== app.themeId) app.setThemeId(s.themeId);
    };
    const off = useSettingsStore.subscribe((s, prev) => {
      if (s.settings !== prev.settings) apply();
    });
    const unsubscribe = getClient().subscribe([TOPIC_GLOBAL], (ev) => {
      if (ev.kind === "SettingsChanged") useSettingsStore.getState().applyRemote(ev.payload.settings);
    });
    void useSettingsStore.getState().load();
    return () => {
      off();
      unsubscribe();
    };
  }, [ready]);
}

function DetachedTitleBar({ route }: { route: DetachedRoute }) {
  const { t } = useT(["layout", "shell"]);
  const def = getPanel(route.panelId);
  const project = useAppStore((s) => s.projects.find((p) => p.id === (route.projectId ?? s.activeProjectId)) ?? null);
  const [maximized, setMaximized] = useState(false);
  const native = isTauri();
  const Icon = def?.icon;

  useEffect(() => {
    if (!native) return;
    let unlisten: (() => void) | undefined;
    const sync = () => void windowControls.isMaximized().then(setMaximized, () => {});
    sync();
    windowControls.onResized(sync).then((u) => (unlisten = u), () => {});
    return () => unlisten?.();
  }, [native]);

  return (
    <header className="titlebar titlebar--detached" data-tauri-drag-region>
      <div className="titlebar__brand" data-tauri-drag-region>
        {Icon && <Icon size={15} className="detached__icon" />}
        <span className="detached__title" data-tauri-drag-region>
          {def?.title ?? route.panelId}
        </span>
        {project && (
          <>
            <span className="titlebar__sep" data-tauri-drag-region>
              /
            </span>
            <span className="titlebar__project" title={project.name} data-tauri-drag-region>
              <span className="titlebar__project-dot" style={{ background: project.color ?? undefined }} />
              <span className="titlebar__project-name" data-tauri-drag-region>
                {project.name}
              </span>
            </span>
          </>
        )}
      </div>
      <div className="titlebar__spacer" data-tauri-drag-region />
      <div className="sec-titlebar-group">
        <ApprovalsBadge />
        <EmergencyStopButton />
      </div>
      <div className="titlebar__controls">
        <button type="button" className="titlebar__ctl detached__redock" title={t("detached.redock")} aria-label={t("detached.redock")} onClick={() => void windowsApi.redockSelf()}>
          <PanelLeftOpen size={15} />
        </button>
        {native && (
          <>
            <button type="button" className="titlebar__ctl" aria-label={t("shell:titleBar.minimize")} onClick={() => void windowControls.minimize()}>
              <Minus size={15} />
            </button>
            <button
              type="button"
              className="titlebar__ctl"
              aria-label={maximized ? t("shell:titleBar.restore") : t("shell:titleBar.maximize")}
              onClick={() => void windowControls.toggleMaximize()}
            >
              {maximized ? <Copy size={13} style={{ transform: "scaleX(-1)" }} /> : <Square size={12} />}
            </button>
            <button type="button" className="titlebar__ctl titlebar__ctl--close" aria-label={t("detached.close")} title={t("detached.close")} onClick={() => void windowControls.close()}>
              <X size={16} />
            </button>
          </>
        )}
      </div>
    </header>
  );
}

function DetachedBody({ route }: { route: DetachedRoute }) {
  const { t } = useT("shell");
  const activeProjectId = useAppStore((s) => s.activeProjectId);
  const def = getPanel(route.panelId);
  if (!def) return <EmptyState title={t("workspace.unknownPanel")} description={route.panelId} />;
  const Component = def.component;
  return (
    <div className="detached__panel panel-host">
      <Component projectId={route.projectId ?? activeProjectId} />
    </div>
  );
}

export function DetachedPanelApp({ route }: { route: DetachedRoute }) {
  const [ready, setReady] = useState(false);
  useThemeEngine(ready);
  useDetachedSettings(ready);
  const boot = useCallback(() => {
    bootstrapCore().then(
      () => setReady(true),
      (e: unknown) => console.error("core bootstrap failed", e),
    );
  }, []);
  useEffect(() => boot(), [boot]);
  useEffect(() => {
    const def = getPanel(route.panelId);
    if (def) document.title = `${def.title} — Orkestr`;
  }, [route.panelId]);

  return (
    <div className="app app--detached">
      <div className="app__backdrop" aria-hidden="true" />
      <DetachedTitleBar route={route} />
      {ready ? (
        <main className="detached__main">
          <DetachedBody route={route} />
        </main>
      ) : (
        <ConnectionScreen onRetry={boot} />
      )}
      <SecurityLayer connected={ready} />
      <Toasts />
    </div>
  );
}
