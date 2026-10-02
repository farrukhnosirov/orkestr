import { EmptyState, Tabs } from "@orkestr/ui";
import { BookOpen, BrainCircuit, FolderOpen, Globe, Scale } from "lucide-react";
import { useState } from "react";

import { useAppStore } from "../../store/app";
import type { PanelProps } from "../types";
import { DecisionLog } from "./DecisionLog";
import { GLOBAL_KINDS, PROJECT_KINDS } from "./kinds";
import { KnowledgeBase } from "./KnowledgeBase";
import "./memory.css";
import { MemoryList } from "./MemoryList";

type Tab = "project" | "global" | "decisions" | "knowledge";

const TABS = [
  { id: "project" as const, label: "Project Memory", icon: <BrainCircuit size={13} /> },
  { id: "global" as const, label: "Global Memory", icon: <Globe size={13} /> },
  { id: "decisions" as const, label: "Decision Log", icon: <Scale size={13} /> },
  { id: "knowledge" as const, label: "Knowledge Base", icon: <BookOpen size={13} /> },
];

function NeedsProject({ what }: { what: string }) {
  return (
    <EmptyState
      icon={<FolderOpen size={26} strokeWidth={1.6} />}
      title="Open a project"
      description={`${what} belongs to a project. Select or create one in the project rail.`}
    />
  );
}

/** Memory panel (§20–21): project & global memory, decision log, knowledge base. */
export default function MemoryPanel({ projectId }: PanelProps) {
  const [tab, setTab] = useState<Tab>("project");
  const connected = useAppStore((s) => s.connection.state === "open");

  let body;
  if (!connected) {
    body = <EmptyState icon={<BrainCircuit size={26} strokeWidth={1.6} />} title="Not connected" description="Waiting for the Orkestr core…" />;
  } else if (tab === "project") {
    body = projectId ? (
      <MemoryList key={projectId} scope={{ scope: "Project", projectId }} kinds={PROJECT_KINDS} />
    ) : (
      <NeedsProject what="Project memory" />
    );
  } else if (tab === "global") {
    body = <MemoryList scope={{ scope: "Global" }} kinds={GLOBAL_KINDS} />;
  } else if (tab === "decisions") {
    body = projectId ? <DecisionLog key={projectId} projectId={projectId} /> : <NeedsProject what="The decision log" />;
  } else {
    body = <KnowledgeBase key={projectId ?? "global"} projectId={projectId} />;
  }

  return (
    <div className="mem-panel">
      <div className="mem-panel__tabs">
        <Tabs aria-label="Memory sections" items={TABS} value={tab} onChange={setTab} />
      </div>
      <div className="mem-panel__body" role="tabpanel">
        {body}
      </div>
    </div>
  );
}
