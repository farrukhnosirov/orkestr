/**
 * Panel registry (§14). Every dockable panel is listed here exactly once; the
 * docking workspace, command palette and layout presets read from it.
 * Phase owners replace a placeholder component with the real panel in place.
 */
import {
  Activity,
  Bot,
  BrainCircuit,
  Bug,
  Code2,
  Coins,
  Database,
  FileCode,
  FolderTree,
  GitBranch,
  Globe,
  LayoutDashboard,
  LayoutGrid,
  ListTodo,
  Network,
  Puzzle,
  Rocket,
  ScrollText,
  Server,
  Sparkles,
  Sparkle,
  SquareTerminal,
  Store,
} from "lucide-react";

import AgentsPanel from "./Agents";
import ArchitecturePanel from "./Architecture";
import BrowserPanel from "./Browser";
import BugDetectivePanel from "./BugDetective";
import ClaudeCodePanel from "./ClaudeCode";
import CodexPanel from "./Codex";
import CodePanel from "./Code";
import CostsPanel from "./Costs";
import DashboardsPanel from "./Dashboards";
import DatabasePanel from "./Database";
import DeploymentsPanel from "./Deployments";
import ExtensionsPanel from "./Extensions";
import FilesPanel from "./Files";
import GitPanel from "./Git";
import LogsPanel from "./Logs";
import ManagerPanel from "./Manager";
import MarketplacePanel from "./Marketplace";
import MemoryPanel from "./Memory";
import MissionControlPanel from "./MissionControl";
import MonitoringPanel from "./Monitoring";
import NodesPanel from "./Nodes";
import TasksPanel from "./Tasks";
import TerminalPanel from "./Terminal";
import VsCodePanel from "./VsCode";
import { t } from "../i18n";
import type shellEn from "../locales/en/shell.json";
import { PLANNED_PHASE } from "./plannedPhases";
import type { PanelComponent, PanelDefinition, PanelIcon } from "./types";

type PanelKey = keyof (typeof shellEn)["panels"];

/** Registry entry whose title/description are localized getters (re-read on every render). */
function panel(id: PanelKey, icon: PanelIcon, phase: number, component: PanelComponent): PanelDefinition {
  return {
    id,
    icon,
    phase,
    component,
    get title() {
      return t(`shell:panels.${id}.title`);
    },
    get description() {
      return t(`shell:panels.${id}.description`);
    },
  };
}

export const PANELS: readonly PanelDefinition[] = [
  panel("manager", Sparkles, 0, ManagerPanel),
  panel("code", FileCode, 0, CodePanel),
  panel("files", FolderTree, 0, FilesPanel),
  panel("terminal", SquareTerminal, 0, TerminalPanel),
  panel("git", GitBranch, 0, GitPanel),
  panel("agents", Bot, 0, AgentsPanel),
  panel("tasks", ListTodo, 0, TasksPanel),
  panel("architecture", Network, 0, ArchitecturePanel),
  panel("browser", Globe, PLANNED_PHASE.browser, BrowserPanel),
  panel("database", Database, PLANNED_PHASE.database, DatabasePanel),
  panel("logs", ScrollText, 0, LogsPanel),
  panel("deployments", Rocket, 0, DeploymentsPanel),
  panel("memory", BrainCircuit, 0, MemoryPanel),
  panel("costs", Coins, 0, CostsPanel),
  // mission-control (Phase 3)
  panel("missionControl", LayoutDashboard, 0, MissionControlPanel),
  panel("monitoring", Activity, 0, MonitoringPanel),
  // Embedded VS Code (VSCodium REH web server + Open VSX extensions).
  panel("vscode", Code2, 0, VsCodePanel),
  // VS Code extensions for the native editor (ADR-0025)
  panel("extensions", Puzzle, 0, ExtensionsPanel),
  // Claude Code chat (official `claude` CLI, stream-json)
  panel("claudeCode", Sparkle, 0, ClaudeCodePanel),
  // Codex chat (official `codex` CLI, ChatGPT subscription, ADR-0047)
  panel("codex", Bot, 0, CodexPanel),
  // multi-node (Phase 6): enrolled nodes, jobs, model distribution
  panel("nodes", Server, 0, NodesPanel),
  // ecosystem (Phase 7): marketplace (WP2), dashboards (WP3), Bug Detective (WP6)
  panel("marketplace", Store, 0, MarketplacePanel),
  panel("dashboards", LayoutGrid, 0, DashboardsPanel),
  panel("bugDetective", Bug, 0, BugDetectivePanel),
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

/** Panel focused when the default layout is applied: the AI Manager is the primary entry point (§1). */
export const DEFAULT_FOCUSED_PANEL = "manager";

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
