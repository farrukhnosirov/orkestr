/**
 * Panel registry (§14). Every dockable panel is listed here exactly once; the
 * docking workspace, command palette and layout presets read from it.
 * Phase owners replace a placeholder component with the real panel in place.
 */
import {
  Activity,
  Bot,
  BrainCircuit,
  Coins,
  Database,
  FileCode,
  FolderTree,
  GitBranch,
  Globe,
  ListTodo,
  Network,
  Rocket,
  ScrollText,
  Sparkles,
  SquareTerminal,
} from "lucide-react";

import AgentsPanel from "./Agents";
import ArchitecturePanel from "./Architecture";
import BrowserPanel from "./Browser";
import CodePanel from "./Code";
import CostsPanel from "./Costs";
import DatabasePanel from "./Database";
import DeploymentsPanel from "./Deployments";
import FilesPanel from "./Files";
import GitPanel from "./Git";
import LogsPanel from "./Logs";
import ManagerPanel from "./Manager";
import MemoryPanel from "./Memory";
import MonitoringPanel from "./Monitoring";
import TasksPanel from "./Tasks";
import TerminalPanel from "./Terminal";
import type { PanelDefinition } from "./types";

export const PANELS: readonly PanelDefinition[] = [
  { id: "manager", title: "Manager", description: "AI Manager chat and plans", icon: Sparkles, phase: 1, component: ManagerPanel },
  { id: "code", title: "Code", description: "Monaco editor", icon: FileCode, phase: 1, component: CodePanel },
  { id: "files", title: "Files", description: "Project file explorer", icon: FolderTree, phase: 1, component: FilesPanel },
  { id: "terminal", title: "Terminal", description: "PTY terminal sessions", icon: SquareTerminal, phase: 1, component: TerminalPanel },
  { id: "git", title: "Git", description: "Status, diffs, branches, worktrees", icon: GitBranch, phase: 0, component: GitPanel },
  { id: "agents", title: "Agents", description: "Agent templates and instances", icon: Bot, phase: 1, component: AgentsPanel },
  { id: "tasks", title: "Tasks", description: "Task board and DAG", icon: ListTodo, phase: 0, component: TasksPanel },
  { id: "architecture", title: "Architecture", description: "Architecture map and decisions", icon: Network, phase: 2, component: ArchitecturePanel },
  { id: "browser", title: "Browser", description: "Browser agent", icon: Globe, phase: 2, component: BrowserPanel },
  { id: "database", title: "Database", description: "Database explorer", icon: Database, phase: 2, component: DatabasePanel },
  { id: "logs", title: "Logs", description: "Live event stream", icon: ScrollText, phase: 0, component: LogsPanel },
  { id: "deployments", title: "Deployments", description: "Environments and deployments", icon: Rocket, phase: 2, component: DeploymentsPanel },
  { id: "memory", title: "Memory", description: "Project memory and knowledge", icon: BrainCircuit, phase: 1, component: MemoryPanel },
  { id: "costs", title: "Costs", description: "Usage, budgets and limits", icon: Coins, phase: 1, component: CostsPanel },
  { id: "monitoring", title: "Monitoring", description: "Mission Control and infrastructure", icon: Activity, phase: 3, component: MonitoringPanel },
];

const BY_ID = new Map(PANELS.map((p) => [p.id, p]));

/** Looks up a panel definition. */
export function getPanel(id: string): PanelDefinition | undefined {
  return BY_ID.get(id);
}

/** Placement of a panel in a layout preset. */
export interface PresetSlot {
  panel: string;
  /** Group anchor: panel id already placed, with direction; omitted = first panel. */
  position?: { referencePanel: string; direction: "left" | "right" | "above" | "below" | "within" };
}

/** Default "Coding" layout preset (§29). */
export const CODING_PRESET: readonly PresetSlot[] = [
  { panel: "code" },
  { panel: "manager", position: { referencePanel: "code", direction: "within" } },
  { panel: "files", position: { referencePanel: "code", direction: "left" } },
  { panel: "agents", position: { referencePanel: "code", direction: "right" } },
  { panel: "tasks", position: { referencePanel: "agents", direction: "within" } },
  { panel: "logs", position: { referencePanel: "code", direction: "below" } },
  { panel: "terminal", position: { referencePanel: "logs", direction: "within" } },
  { panel: "git", position: { referencePanel: "logs", direction: "within" } },
];
