import { EmptyState, Tabs } from "@orkestr/ui";
import { BookOpen, BrainCircuit, FolderOpen, Globe, Scale } from "lucide-react";
import { type ReactElement, useState } from "react";

import { useT } from "../../i18n";
import { useAppStore } from "../../store/app";
import type { PanelProps } from "../types";
import { DecisionLog } from "./DecisionLog";
import { GLOBAL_KINDS, PROJECT_KINDS } from "./kinds";
import { KnowledgeBase } from "./KnowledgeBase";
import "./memory.css";
import { MemoryList } from "./MemoryList";

type Tab = "project" | "global" | "decisions" | "knowledge";

const TAB_ICONS: Record<Tab, ReactElement> = {
  project: <BrainCircuit size={13} />,
  global: <Globe size={13} />,
  decisions: <Scale size={13} />,
  knowledge: <BookOpen size={13} />,
};

function NeedsProject({ what }: { what: "projectMemory" | "decisions" }) {
  const { t } = useT("memory");
  return <EmptyState icon={<FolderOpen size={26} strokeWidth={1.6} />} title={t("needsProject.title")} description={t(`needsProject.${what}`)} />;
}

/** Memory panel (§20–21): project & global memory, decision log, knowledge base. */
export default function MemoryPanel({ projectId }: PanelProps) {
  const { t } = useT("memory");
  const [tab, setTab] = useState<Tab>("project");
  const tabs = (Object.keys(TAB_ICONS) as Tab[]).map((id) => ({ id, label: t(`tabs.${id}`), icon: TAB_ICONS[id] }));
  const connected = useAppStore((s) => s.connection.state === "open");

  let body;
  if (!connected) {
    body = <EmptyState icon={<BrainCircuit size={26} strokeWidth={1.6} />} title={t("notConnected.title")} description={t("notConnected.description")} />;
  } else if (tab === "project") {
    body = projectId ? (
      <MemoryList key={projectId} scope={{ scope: "Project", projectId }} kinds={PROJECT_KINDS} />
    ) : (
      <NeedsProject what="projectMemory" />
    );
  } else if (tab === "global") {
    body = <MemoryList scope={{ scope: "Global" }} kinds={GLOBAL_KINDS} />;
  } else if (tab === "decisions") {
    body = projectId ? <DecisionLog key={projectId} projectId={projectId} /> : <NeedsProject what="decisions" />;
  } else {
    body = <KnowledgeBase key={projectId ?? "global"} projectId={projectId} />;
  }

  return (
    <div className="mem-panel">
      <div className="mem-panel__tabs">
        <Tabs aria-label={t("tabs.label")} items={tabs} value={tab} onChange={setTab} />
      </div>
      <div className="mem-panel__body" role="tabpanel">
        {body}
      </div>
    </div>
  );
}
