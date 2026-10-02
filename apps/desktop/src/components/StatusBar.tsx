import type { ConnectionState } from "@orkestr/client";
import { Kbd, type Status, StatusDot } from "@orkestr/ui";
import { Cpu, Radio } from "lucide-react";

import { useFormat, useT } from "../i18n";
import { useActiveProject, useAppStore } from "../store/app";

const DOT: Record<ConnectionState, Status> = {
  idle: "idle",
  connecting: "connecting",
  authenticating: "connecting",
  open: "online",
  reconnecting: "warning",
  closed: "error",
};

/** Bottom status bar: core connection, mode, version, active project. */
export function StatusBar() {
  const connection = useAppStore((s) => s.connection);
  const lastSeq = useAppStore((s) => s.lastSeq);
  const project = useActiveProject();
  const { t } = useT("shell");
  const fmt = useFormat();
  const pulsing = connection.state === "connecting" || connection.state === "reconnecting" || connection.state === "authenticating";

  return (
    <footer className="statusbar">
      <div className="statusbar__item" title={connection.error ?? connection.url ?? undefined}>
        <StatusDot status={DOT[connection.state]} pulse={pulsing} />
        <span>{t(`connection.states.${connection.state}`)}</span>
      </div>
      {connection.mode && (
        <div className="statusbar__item statusbar__item--muted">
          <Cpu size={12} />
          <span>
            {connection.mode === "embedded"
              ? t("statusBar.modeEmbedded")
              : connection.mode === "external"
                ? t("statusBar.modeExternal")
                : t("statusBar.modeBrowser")}
            {connection.serverVersion ? ` ${t("statusBar.version", { version: connection.serverVersion })}` : ""}
            {connection.startupMs != null && connection.mode === "embedded"
              ? ` · ${t("statusBar.startup", { ms: fmt.integer(connection.startupMs) })}`
              : ""}
          </span>
        </div>
      )}
      <div className="statusbar__item statusbar__item--muted" title={t("statusBar.seqTitle")}>
        <Radio size={12} />
        <span>{t("statusBar.seq", { seq: lastSeq })}</span>
      </div>
      <div className="statusbar__spacer" />
      {project && <div className="statusbar__item statusbar__item--muted">{project.name}</div>}
      <div className="statusbar__item statusbar__item--muted">
        <Kbd>Ctrl</Kbd>
        <Kbd>K</Kbd>
        <span>{t("statusBar.commands")}</span>
      </div>
    </footer>
  );
}
