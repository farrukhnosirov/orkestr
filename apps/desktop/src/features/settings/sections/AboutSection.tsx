import { Badge, Button, Spinner } from "@orkestr/ui";
import { ScrollText } from "lucide-react";
import { useEffect } from "react";

import { Logo } from "../../../components/Logo";
import { workspaceActions } from "../../../components/workspaceActions";
import { useAppStore } from "../../../store/app";
import { formatDuration } from "../controls";
import type { SettingsSectionProps } from "../sections";
import { useSettingsStore } from "../settingsStore";
import { useSystemStore } from "../systemStore";

/** Settings → About. */
export function AboutSection(_props: SettingsSectionProps) {
  const system = useSystemStore((s) => s.system);
  const loadSystem = useSystemStore((s) => s.loadSystem);
  const connection = useAppStore((s) => s.connection);
  const closeSettings = useSettingsStore((s) => s.closeSettings);

  useEffect(() => {
    void loadSystem();
  }, [loadSystem]);

  const rows: Array<[string, string]> = system
    ? [
        ["Core version", system.version],
        ["Protocol version", String(system.protocolVersion)],
        [
          "Core mode",
          connection.mode === "embedded" ? "Embedded in the desktop app" : connection.mode === "external" ? "orkestrd daemon" : "Browser",
        ],
        ["Uptime", formatDuration(Date.now() - new Date(system.startedAt).getTime())],
        ["Node id", system.nodeId],
        ["Data directory", system.dataDir],
        ["Process id", String(system.pid)],
        ["Shells", system.shells.join(", ") || "—"],
      ]
    : [];

  return (
    <div className="set-section">
      <div className="about-hero">
        <Logo size={48} />
        <div>
          <h2 className="about-hero__title">Orkestr</h2>
          <p className="about-hero__sub">AI Development OS · local-first, your keys, your machine</p>
          {system?.recoveredFromCrash && <Badge tone="warning">Recovered from an unclean shutdown</Badge>}
        </div>
      </div>
      {!system ? (
        <div className="set-loading">
          <Spinner size={16} /> Loading…
        </div>
      ) : (
        <dl className="about-grid">
          {rows.map(([k, v]) => (
            <div key={k} className="about-grid__row">
              <dt>{k}</dt>
              <dd className={k === "Node id" || k === "Data directory" ? "about-grid__mono" : undefined}>{v}</dd>
            </div>
          ))}
        </dl>
      )}
      <div className="about-actions">
        <Button
          leftIcon={<ScrollText size={14} />}
          onClick={() => {
            closeSettings();
            workspaceActions.openPanel("logs");
          }}
        >
          Open logs
        </Button>
      </div>
      <p className="set-muted set-muted--small">
        Licensed MIT OR Apache-2.0. No telemetry is collected; nothing leaves this machine unless you configure a
        provider.
      </p>
    </div>
  );
}
