import type { ConnectionState } from "@orkestr/client";
import { Kbd, type Status, StatusDot } from "@orkestr/ui";
import { Cpu, Radio } from "lucide-react";

import { useActiveProject, useAppStore } from "../store/app";

const LABEL: Record<ConnectionState, string> = {
  idle: "Starting",
  connecting: "Connecting to core…",
  authenticating: "Authenticating…",
  open: "Core connected",
  reconnecting: "Reconnecting…",
  closed: "Disconnected",
};

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
  const pulsing = connection.state === "connecting" || connection.state === "reconnecting" || connection.state === "authenticating";

  return (
    <footer className="statusbar">
      <div className="statusbar__item" title={connection.error ?? connection.url ?? undefined}>
        <StatusDot status={DOT[connection.state]} pulse={pulsing} />
        <span>{LABEL[connection.state]}</span>
      </div>
      {connection.mode && (
        <div className="statusbar__item statusbar__item--muted">
          <Cpu size={12} />
          <span>
            {connection.mode === "embedded" ? "Embedded core" : connection.mode === "external" ? "orkestrd" : "Browser"}
            {connection.serverVersion ? ` v${connection.serverVersion}` : ""}
            {connection.startupMs != null && connection.mode === "embedded" ? ` · ${connection.startupMs} ms start` : ""}
          </span>
        </div>
      )}
      <div className="statusbar__item statusbar__item--muted" title="Last event sequence number">
        <Radio size={12} />
        <span>seq {lastSeq}</span>
      </div>
      <div className="statusbar__spacer" />
      {project && <div className="statusbar__item statusbar__item--muted">{project.name}</div>}
      <div className="statusbar__item statusbar__item--muted">
        <Kbd>Ctrl</Kbd>
        <Kbd>K</Kbd>
        <span>Commands</span>
      </div>
    </footer>
  );
}
