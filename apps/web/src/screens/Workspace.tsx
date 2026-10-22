import type { DeviceRecord } from "@orkestr/client";
import { Button, Tabs } from "@orkestr/ui";
import { useState } from "react";
import { useTranslation } from "react-i18next";

import { EditorPanel } from "../panels/EditorPanel";
import { ApprovalsPanel, ProjectsPanel, TasksPanel } from "../panels/lists";
import { SettingsPanel } from "../panels/SettingsPanel";
import { TerminalPanel } from "../panels/TerminalPanel";
import type { RemoteClient } from "../remote";

const TABS = ["projects", "tasks", "approvals", "terminal", "editor", "settings"] as const;
type Tab = (typeof TABS)[number];

/** Compact remote workspace over the E2E channel. */
export function Workspace({
  client,
  device,
  onLogout,
  onForget,
}: {
  client: RemoteClient;
  device: DeviceRecord;
  onLogout: () => void;
  onForget: () => void;
}) {
  const { t } = useTranslation();
  const [tab, setTab] = useState<Tab>("projects");
  const [projectId, setProjectId] = useState<string | null>(null);

  return (
    <div className="web-app">
      <header className="web-header">
        <strong>{t("app.title")}</strong>
        <span className="web-muted">{device.coreId}</span>
        <Button size="sm" variant="ghost" onClick={onLogout}>
          {t("app.logout")}
        </Button>
      </header>
      <Tabs
        aria-label={t("app.sections")}
        items={TABS.map((id) => ({ id, label: t(`tabs.${id}`) }))}
        value={tab}
        onChange={setTab}
      />
      <div className="web-body">
        {tab === "projects" && (
          <ProjectsPanel
            client={client}
            selected={projectId}
            onSelect={(id) => {
              setProjectId(id);
              setTab("tasks");
            }}
          />
        )}
        {tab === "tasks" && <TasksPanel client={client} projectId={projectId} />}
        {tab === "approvals" && <ApprovalsPanel client={client} />}
        {tab === "terminal" && <TerminalPanel client={client} projectId={projectId} />}
        {tab === "editor" && <EditorPanel client={client} projectId={projectId} />}
        {tab === "settings" && <SettingsPanel client={client} device={device} onForget={onForget} />}
      </div>
    </div>
  );
}
