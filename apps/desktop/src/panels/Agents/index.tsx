/**
 * Agents panel (§6, §30): live agent instances with state, task, model,
 * worktree, usage and elapsed time; a detail view with the streamed
 * transcript (messages, collapsible tool calls, inline approvals); the Spawn
 * Agent dialog; and the Templates tab. All data comes from the core over RPC
 * (`agents.*`, `templates.*`) and live events.
 */
import "./agents.css";

import { Badge, Button, EmptyState, IconButton, Spinner, Tabs } from "@orkestr/ui";
import { Bot, FolderOpen, LayoutTemplate, Plus, RefreshCw, TriangleAlert } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import { useProjectTasks } from "../Tasks/data";
import type { PanelProps } from "../types";
import { AgentDetail } from "./AgentDetail";
import { AgentList } from "./AgentList";
import { useProjectAgents, useTemplates } from "./data";
import { isActive } from "./model";
import { Toasts } from "./parts";
import { SpawnDialog } from "./SpawnDialog";
import { type AgentsTab, useAgentsUi } from "./store";
import { TemplatesTab } from "./TemplatesTab";

const TABS: { id: AgentsTab; label: string; icon: JSX.Element }[] = [
  { id: "agents", label: "Agents", icon: <Bot size={14} /> },
  { id: "templates", label: "Templates", icon: <LayoutTemplate size={14} /> },
];

function useNow(active: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return undefined;
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, [active]);
  return now;
}

export default function AgentsPanel({ projectId }: PanelProps) {
  const { agents, loading, error, reload } = useProjectAgents(projectId);
  const { tasks } = useProjectTasks(projectId);
  const templates = useTemplates();
  const tab = useAgentsUi((s) => s.tab);
  const setTab = useAgentsUi((s) => s.setTab);
  const selectedId = useAgentsUi((s) => s.selectedAgentId);
  const select = useAgentsUi((s) => s.select);
  const openSpawn = useAgentsUi((s) => s.openSpawn);
  const setActiveAgents = useAgentsUi((s) => s.setActiveAgents);
  const activeIds = useMemo(() => agents.filter((a) => isActive(a.state)).map((a) => a.id), [agents]);
  const now = useNow(activeIds.length > 0);

  useEffect(() => setActiveAgents(activeIds), [activeIds, setActiveAgents]);
  useEffect(() => {
    if (!selectedId && agents[0]) select(agents[0].id);
  }, [agents, selectedId, select]);

  if (!projectId) {
    return (
      <div className="agents-panel agents-panel--center">
        <EmptyState
          icon={<FolderOpen size={26} strokeWidth={1.6} />}
          title="No project open"
          description="Open or create a project to run agents on its tasks."
        />
      </div>
    );
  }

  const selected = agents.find((a) => a.id === selectedId) ?? null;
  const selectedTask = selected?.taskId ? (tasks.find((t) => t.id === selected.taskId)?.title ?? null) : null;

  let content: JSX.Element;
  if (tab === "templates") {
    content = <TemplatesTab templates={templates.templates} loading={templates.loading} error={templates.error} reload={templates.reload} />;
  } else if (loading && agents.length === 0) {
    content = (
      <div className="agents-loading">
        <Spinner size={18} /> Loading agents…
      </div>
    );
  } else if (error) {
    content = (
      <EmptyState
        icon={<TriangleAlert size={26} strokeWidth={1.6} />}
        title="Could not load agents"
        description={error}
        action={
          <Button leftIcon={<RefreshCw size={14} />} onClick={reload}>
            Retry
          </Button>
        }
      />
    );
  } else if (agents.length === 0) {
    content = (
      <EmptyState
        icon={<Bot size={26} strokeWidth={1.6} />}
        title="No agents yet"
        description="Spawn an agent from a template. Each agent works in isolation — its own worktree, budget, turn limit and stop button — and asks before anything dangerous."
        action={
          <Button variant="primary" leftIcon={<Plus size={14} />} onClick={() => openSpawn()}>
            Spawn agent
          </Button>
        }
      />
    );
  } else {
    content = (
      <div className="agents-split">
        <div className="agents-scroll agents-split__list">
          <AgentList agents={agents} tasks={tasks} now={now} />
        </div>
        {selected ? (
          <AgentDetail key={selected.id} agent={selected} taskTitle={selectedTask} now={now} />
        ) : (
          <div className="agents-panel--center">
            <EmptyState title="Select an agent" description="Its live transcript appears here." />
          </div>
        )}
      </div>
    );
  }

  return (
    <div className="agents-panel">
      <div className="agents-toolbar">
        <Button size="sm" variant="primary" leftIcon={<Plus size={14} />} onClick={() => openSpawn()}>
          Spawn agent
        </Button>
        <Tabs aria-label="Agents view" items={TABS} value={tab} onChange={setTab} />
        <div className="agents-toolbar__spacer" />
        <div className="agents-toolbar__stats" aria-label="Agent summary">
          <Badge>{agents.length} agents</Badge>
          {activeIds.length > 0 && <Badge tone="info">{activeIds.length} running</Badge>}
        </div>
        <IconButton label="Reload agents" icon={<RefreshCw size={14} />} onClick={reload} tooltip="bottom" />
      </div>
      <div className="agents-body">{content}</div>
      <SpawnDialog projectId={projectId} templates={templates.templates} tasks={tasks} />
      <Toasts />
    </div>
  );
}
