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
import type { PanelCategory, PanelComponent, PanelDefinition, PanelIcon } from "./types";

type PanelKey = keyof (typeof shellEn)["panels"];

/** Splits a localized comma-separated tag list. */
export function splitTags(text: string): string[] {
  return text
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
}

/**
 * Registry entry whose texts are localized getters (re-read on every render).
 * `summary` / `details` / `tags` feed the Window Library; the guard test
 * (`panels/library.test.ts`) requires them, a category and previews for every panel.
 */
function panel(id: PanelKey, icon: PanelIcon, phase: number, component: PanelComponent, category: PanelCategory): PanelDefinition {
  return {
    id,
    icon,
    phase,
    component,
    category,
    get summary() {
      return t(`windowLibrary:panels.${id}.summary`);
    },
    get details() {
      return t(`windowLibrary:panels.${id}.details`);
    },
    get tags() {
      return splitTags(t(`windowLibrary:panels.${id}.tags`));
    },
    get title() {
      return t(`shell:panels.${id}.title`);
    },
    get description() {
      return t(`shell:panels.${id}.description`);
    },
  };
}

export const PANELS: readonly PanelDefinition[] = [
  panel("manager", Sparkles, 0, ManagerPanel, "work"),
  panel("code", FileCode, 0, CodePanel, "code"),
  panel("files", FolderTree, 0, FilesPanel, "code"),
  panel("terminal", SquareTerminal, 0, TerminalPanel, "code"),
  panel("git", GitBranch, 0, GitPanel, "code"),
  panel("agents", Bot, 0, AgentsPanel, "ai"),
  panel("tasks", ListTodo, 0, TasksPanel, "work"),
  panel("architecture", Network, 0, ArchitecturePanel, "quality"),
  panel("browser", Globe, PLANNED_PHASE.browser, BrowserPanel, "ai"),
  panel("database", Database, PLANNED_PHASE.database, DatabasePanel, "ops"),
  panel("logs", ScrollText, 0, LogsPanel, "monitoring"),
  panel("deployments", Rocket, 0, DeploymentsPanel, "ops"),
  panel("memory", BrainCircuit, 0, MemoryPanel, "ai"),
  panel("costs", Coins, 0, CostsPanel, "monitoring"),
  // mission-control (Phase 3)
  panel("missionControl", LayoutDashboard, 0, MissionControlPanel, "work"),
  panel("monitoring", Activity, 0, MonitoringPanel, "monitoring"),
  // Embedded VS Code (VSCodium REH web server + Open VSX extensions).
  panel("vscode", Code2, 0, VsCodePanel, "code"),
  // VS Code extensions for the native editor (ADR-0025)
  panel("extensions", Puzzle, 0, ExtensionsPanel, "ecosystem"),
  // Claude Code chat (official `claude` CLI, stream-json)
  panel("claudeCode", Sparkle, 0, ClaudeCodePanel, "ai"),
  // multi-node (Phase 6): enrolled nodes, jobs, model distribution
  panel("nodes", Server, 0, NodesPanel, "ops"),
  // ecosystem (Phase 7): marketplace (WP2), dashboards (WP3), Bug Detective (WP6)
  panel("marketplace", Store, 0, MarketplacePanel, "ecosystem"),
  panel("dashboards", LayoutGrid, 0, DashboardsPanel, "work"),
  panel("bugDetective", Bug, 0, BugDetectivePanel, "quality"),
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
