import { Badge, Button, Spinner } from "@orkestr/ui";
import { ScrollText } from "lucide-react";
import { useEffect } from "react";

import { Logo } from "../../../components/Logo";
import { workspaceActions } from "../../../components/workspaceActions";
import { useFormat, useT } from "../../../i18n";
import { useAppStore } from "../../../store/app";
import type { SettingsSectionProps } from "../sections";
import { useSettingsStore } from "../settingsStore";
import { useSystemStore } from "../systemStore";

/** Settings → About. */
export function AboutSection(_props: SettingsSectionProps) {
  const system = useSystemStore((s) => s.system);
  const loadSystem = useSystemStore((s) => s.loadSystem);
  const connection = useAppStore((s) => s.connection);
  const closeSettings = useSettingsStore((s) => s.closeSettings);
  const { t } = useT(["settings", "common"]);
  const fmt = useFormat();

  useEffect(() => {
    void loadSystem();
  }, [loadSystem]);

  /** `[id, label, value]`; ids mark monospaced values. */
  const rows: Array<[string, string, string]> = system
    ? [
        ["version", t("about.coreVersion"), system.version],
        ["protocol", t("about.protocolVersion"), String(system.protocolVersion)],
        [
          "mode",
          t("about.coreMode"),
          connection.mode === "embedded"
            ? t("about.modes.embedded")
            : connection.mode === "external"
              ? t("about.modes.external")
              : t("about.modes.browser"),
        ],
        ["uptime", t("about.uptime"), fmt.duration(Date.now() - new Date(system.startedAt).getTime())],
        ["nodeId", t("about.nodeId"), system.nodeId],
        ["dataDir", t("about.dataDir"), system.dataDir],
        ["pid", t("about.pid"), String(system.pid)],
        ["shells", t("about.shells"), system.shells.join(", ") || "—"],
      ]
    : [];

  return (
    <div className="set-section">
      <div className="about-hero">
        <Logo size={48} />
        <div>
          <h2 className="about-hero__title">Orkestr</h2>
          <p className="about-hero__sub">{t("about.tagline")}</p>
          {system?.recoveredFromCrash && <Badge tone="warning">{t("about.recovered")}</Badge>}
        </div>
      </div>
      {!system ? (
        <div className="set-loading">
          <Spinner size={16} /> {t("common:state.loading")}
        </div>
      ) : (
        <dl className="about-grid">
          {rows.map(([id, k, v]) => (
            <div key={id} className="about-grid__row">
              <dt>{k}</dt>
              <dd className={id === "nodeId" || id === "dataDir" ? "about-grid__mono" : undefined}>{v}</dd>
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
          {t("about.openLogs")}
        </Button>
      </div>
      <p className="set-muted set-muted--small">
        {t("about.license")}
      </p>
    </div>
  );
}
