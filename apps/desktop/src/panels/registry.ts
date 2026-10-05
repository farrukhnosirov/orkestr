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
  LayoutDashboard,
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
import MissionControlPanel from "./MissionControl";
import MonitoringPanel from "./Monitoring";
import TasksPanel from "./Tasks";
import TerminalPanel from "./Terminal";
import { t } from "../i18n";
import type shellEn from "../locales/en/shell.json";
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
  panel("architecture", Network, 2, ArchitecturePanel),
  panel("browser", Globe, 2, BrowserPanel),
  panel("database", Database, 2, DatabasePanel),
  panel("logs", ScrollText, 0, LogsPanel),
  panel("deployments", Rocket, 2, DeploymentsPanel),
  panel("memory", BrainCircuit, 0, MemoryPanel),
  panel("costs", Coins, 0, CostsPanel),
  // mission-control (Phase 3)
  panel("missionControl", LayoutDashboard, 0, MissionControlPanel),
  panel("monitoring", Activity, 0, MonitoringPanel),
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
