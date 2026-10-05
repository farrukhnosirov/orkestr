/**
 * Mission Control widget registry — the foundation of the Widget SDK and
 * Dashboard Builder (§48). A widget is declared by a manifest: id, title,
 * default size and allowed sizes, and its data binding (the RPC methods and
 * event kinds it reads). Widgets never get filesystem or secret access; they
 * read data only through `@orkestr/client` (§48: no automatic access).
 *
 * Plugin widgets (Phase 8) will register manifests with the same shape via
 * `registerWidget`; the arrangement logic validates against this list.
 */
import type { EventKindName, MethodName } from "@orkestr/protocol";
import type { LucideIcon } from "lucide-react";
import {
  Activity,
  BellRing,
  Bot,
  Coins,
  FolderKanban,
  Gauge,
  Network,
  Rocket,
  Server,
  ShieldCheck,
  TestTube2,
} from "lucide-react";
import type { ComponentType } from "react";

import { t } from "../../../i18n";
import type { PlacementDefaults, WidgetSize } from "../arrangement";
import type { WidgetProps } from "../context";
import { AgentsWidget } from "./agents";
import { TaskGraphWidget } from "./graph";
import { NodesWidget, ProvidersWidget } from "./infra";
import { BudgetsWidget } from "./money";
import { AlertsWidget, ApprovalsWidget, DeploymentsWidget, QualityWidget } from "./ops";
import { OverviewWidget, ProjectsWidget } from "./overview";

/** What a widget reads (declared, so a Dashboard Builder can reason about it). */
export interface WidgetDataBinding {
  methods: readonly MethodName[];
  events: readonly EventKindName[];
}

/** Manifest of a widget (§48). */
export interface WidgetManifest extends PlacementDefaults {
  id: string;
  /** Localized title (getter: follows live language switches). */
  readonly title: string;
  icon: LucideIcon;
  dataBinding: WidgetDataBinding;
  component: ComponentType<WidgetProps>;
}

type WidgetKey = "overview" | "projects" | "agents" | "taskGraph" | "nodes" | "providers" | "budgets" | "approvals" | "quality" | "deployments" | "alerts";

function widget(
  id: WidgetKey,
  icon: LucideIcon,
  size: WidgetSize,
  sizes: readonly WidgetSize[],
  dataBinding: WidgetDataBinding,
  component: ComponentType<WidgetProps>,
): WidgetManifest {
  return {
    id,
    icon,
    size,
    sizes,
    dataBinding,
    component,
    get title() {
      return t(`missionControl:widgets.${id}`);
    },
  };
}

const BUILTIN: WidgetManifest[] = [
  widget("overview", Gauge, "full", ["xl", "full"], { methods: ["dashboard.snapshot"], events: ["AgentStateChanged", "TaskStatusChanged"] }, OverviewWidget),
  widget("agents", Bot, "xl", ["l", "xl", "full"], { methods: ["agents.list"], events: ["AgentSpawned", "AgentStateChanged", "AgentUpdated", "AgentStopped"] }, AgentsWidget),
  widget("approvals", ShieldCheck, "m", ["m", "l"], { methods: ["approvals.list", "approvals.resolve"], events: ["ApprovalRequested", "ApprovalResolved"] }, ApprovalsWidget),
  widget("projects", FolderKanban, "l", ["m", "l", "xl"], { methods: ["dashboard.snapshot"], events: ["ProjectCreated", "ProjectUpdated"] }, ProjectsWidget),
  widget("taskGraph", Network, "l", ["l", "xl", "full"], { methods: ["tasks.graph", "manager.status"], events: ["TaskStatusChanged", "ManagerRunUpdated"] }, TaskGraphWidget),
  widget("nodes", Server, "l", ["l", "xl", "full"], { methods: ["nodes.list", "nodes.resources"], events: ["NodeStatus"] }, NodesWidget),
  widget("providers", Activity, "m", ["m", "l"], { methods: ["dashboard.snapshot"], events: ["ProviderStatusChanged", "ProviderRateLimited"] }, ProvidersWidget),
  widget("budgets", Coins, "m", ["m", "l", "xl"], { methods: ["dashboard.snapshot", "budgets.list"], events: ["BudgetThreshold"] }, BudgetsWidget),
  widget("alerts", BellRing, "m", ["m", "l"], { methods: ["events.replay"], events: ["BuildFailed", "ProviderRateLimited", "BudgetThreshold", "AgentStateChanged"] }, AlertsWidget),
  widget("quality", TestTube2, "m", ["m", "l"], { methods: ["events.replay"], events: ["BuildStarted", "BuildFailed", "BuildSucceeded", "TestRunFinished"] }, QualityWidget),
  widget("deployments", Rocket, "s", ["s", "m"], { methods: [], events: ["DeploymentFinished"] }, DeploymentsWidget),
];

const registry: WidgetManifest[] = [...BUILTIN];

/** All registered widgets in default order. */
export function widgets(): readonly WidgetManifest[] {
  return registry;
}

export function getWidget(id: string): WidgetManifest | undefined {
  return registry.find((w) => w.id === id);
}

/** Registers an extra widget (plugins, later phases). Ids must be unique. */
export function registerWidget(manifest: WidgetManifest): void {
  if (registry.some((w) => w.id === manifest.id)) throw new Error(`widget ${manifest.id} registered twice`);
  if (!manifest.sizes.includes(manifest.size)) throw new Error(`widget ${manifest.id}: default size not in sizes`);
  registry.push(manifest);
}
