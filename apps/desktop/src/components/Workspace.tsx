import { Button, EmptyState } from "@orkestr/ui";
import {
  type DockviewApi,
  DockviewReact,
  type DockviewMessages,
  type DockviewReadyEvent,
  type DockviewTheme,
  type IDockviewPanelHeaderProps,
  type IDockviewPanelProps,
  type IWatermarkPanelProps,
} from "dockview-react";
import { LayoutGrid, X } from "lucide-react";
import { type FunctionComponent, useCallback, useEffect, useRef } from "react";

import { t as translate, useLanguage, useT } from "../i18n";
import { CODING_PRESET, DEFAULT_FOCUSED_PANEL, getPanel, PANELS } from "../panels/registry";
import { useAppStore } from "../store/app";
import { registerWorkspace } from "./workspaceActions";

// v2: the AI Manager became the default focused panel (Phase 1).
const LAYOUT_KEY = "orkestr.layout.v3";

type DockPosition = "top" | "bottom" | "left" | "right" | "center";
const dockSide = (p: DockPosition) => (p === "center" ? "" : translate(`shell:dock.position.${p}`));

/** Screen-reader announcements of dockview, localized (read on every announcement, so they follow a live switch). */
const DOCK_MESSAGES: DockviewMessages = {
  panelOpened: (title) => translate("shell:dock.panelOpened", { title }),
  panelClosed: (title) => translate("shell:dock.panelClosed", { title }),
  groupMaximized: (title) => translate("shell:dock.groupMaximized", { title }),
  groupRestored: (title) => translate("shell:dock.groupRestored", { title }),
  groupFloated: (title) => translate("shell:dock.groupFloated", { title }),
  groupDocked: (title) => translate("shell:dock.groupDocked", { title }),
  groupPoppedOut: (title) => translate("shell:dock.groupPoppedOut", { title }),
  closeTab: (title) => translate("shell:dock.closeTab", { title }),
  closeTabPlain: () => translate("shell:dock.closeTabPlain"),
  movePickTarget: (source, target, current, total) => translate("shell:dock.movePickTarget", { source, target, current, total }),
  movePickEdge: (position, target) =>
    translate("shell:dock.movePickEdge", {
      where:
        position === "center"
          ? translate("shell:dock.edgeCenter", { target })
          : translate("shell:dock.edgeSplit", { position: dockSide(position), target }),
    }),
  moveCommitted: (source, target, position) =>
    position === "center"
      ? translate("shell:dock.committedCenter", { source, target })
      : translate("shell:dock.committedSplit", { source, target, position: dockSide(position) }),
  moveCancelled: () => translate("shell:dock.moveCancelled"),
  moveNotAllowed: () => translate("shell:dock.moveNotAllowed"),
  moveFloated: (source) => translate("shell:dock.moveFloated", { source }),
};
/** dockview theme bound to the Orkestr tokens (see `.dockview-theme-nebula` in app.css). */
const NEBULA_DOCK_THEME: DockviewTheme = {
  name: "nebula",
  className: "dockview-theme-nebula",
  colorScheme: "dark",
  gap: 6,
};

/** Renders a registry panel with the active project. */
function PanelHost({ panelId }: { panelId: string }) {
  const projectId = useAppStore((s) => s.activeProjectId);
  const def = getPanel(panelId);
  const { t } = useT("shell");
  if (!def) return <EmptyState title={t("workspace.unknownPanel")} description={panelId} />;
  const Component = def.component;
  return (
    <div className="panel-host">
      <Component projectId={projectId} />
    </div>
  );
}

/** dockview component map generated from the registry. */
const COMPONENTS: Record<string, FunctionComponent<IDockviewPanelProps>> = Object.fromEntries(
  PANELS.map((p) => {
    const C: FunctionComponent<IDockviewPanelProps> = () => <PanelHost panelId={p.id} />;
    C.displayName = `Panel(${p.id})`;
    return [p.id, C];
  }),
);

/** Tab with the panel icon and a close button. */
function PanelTab({ api, params }: IDockviewPanelHeaderProps) {
  const def = getPanel((params as { panelId?: string } | undefined)?.panelId ?? api.id);
  const Icon = def?.icon;
  const { t } = useT("shell");
  // Registry titles are localized getters: prefer them over the title saved in the layout.
  const title = def?.title ?? api.title ?? api.id;
  return (
    <div className="dock-tab" onMouseDown={(e) => e.button === 1 && api.close()}>
      {Icon && <Icon size={13} strokeWidth={2} className="dock-tab__icon" />}
      <span className="dock-tab__title">{title}</span>
      <button
        type="button"
        className="dock-tab__close"
        aria-label={t("workspace.closeTab", { title })}
        onMouseDown={(e) => e.stopPropagation()}
        onClick={(e) => {
          e.stopPropagation();
          api.close();
        }}
      >
        <X size={12} />
      </button>
    </div>
  );
}

function Watermark(_props: IWatermarkPanelProps) {
  const { t } = useT("shell");
  return (
    <EmptyState
      icon={<LayoutGrid size={26} strokeWidth={1.6} />}
      title={t("workspace.emptyTitle")}
      description={t("workspace.emptyDescription")}
      action={
        <Button variant="primary" onClick={() => applyPreset(currentApi)}>
          {t("workspace.restoreLayout")}
        </Button>
      }
    />
  );
}

let currentApi: DockviewApi | null = null;

function applyPreset(api: DockviewApi | null): void {
  if (!api) return;
  api.clear();
  for (const slot of CODING_PRESET) {
    const def = getPanel(slot.panel);
    if (!def) continue;
    api.addPanel({
      id: def.id,
      component: def.id,
      tabComponent: "ok",
      title: def.title,
      params: { panelId: def.id },
      ...(slot.position ? { position: slot.position } : {}),
    });
  }
  // Proportions: the conversation/editor column gets the most room; the
  // explorer and the agents/tasks column are side bars and the bottom dock
  // (logs/terminal/git) a strip, so the Manager chat is never squeezed.
  if (api.width > 0 && api.height > 0) {
    api.getPanel("files")?.group.api.setSize({ width: Math.round(api.width * 0.2) });
    api.getPanel("agents")?.group.api.setSize({ width: Math.round(api.width * 0.27) });
    api.getPanel("logs")?.group.api.setSize({ height: Math.round(api.height * 0.3) });
  }
  api.getPanel("logs")?.api.setActive();
  // Last: the focused group/tab of the default layout is the AI Manager.
  api.getPanel(DEFAULT_FOCUSED_PANEL)?.api.setActive();
}

/** Whether a serialized layout only references known panels. */
function layoutIsValid(layout: unknown): boolean {
  if (typeof layout !== "object" || layout === null) return false;
  const panels = (layout as { panels?: Record<string, { contentComponent?: string }> }).panels;
  if (!panels || typeof panels !== "object") return false;
  const values = Object.values(panels);
  return values.length > 0 && values.every((p) => typeof p.contentComponent === "string" && !!getPanel(p.contentComponent));
}

/** The docking workspace (§14, §28). */
export function Workspace() {
  const apiRef = useRef<DockviewApi | null>(null);
  const saveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const openPanel = useCallback((panelId: string) => {
    const api = apiRef.current;
    const def = getPanel(panelId);
    if (!api || !def) return;
    const existing = api.getPanel(panelId);
    if (existing) {
      existing.api.setActive();
      return;
    }
    const anchor = api.activePanel;
    api.addPanel({
      id: def.id,
      component: def.id,
      tabComponent: "ok",
      title: def.title,
      params: { panelId: def.id },
      ...(anchor ? { position: { referencePanel: anchor.id, direction: "within" as const } } : {}),
    });
  }, []);

  const resetLayout = useCallback(() => {
    applyPreset(apiRef.current);
  }, []);

  const toggleMaximize = useCallback(() => {
    const api = apiRef.current;
    if (!api) return;
    if (api.hasMaximizedGroup()) {
      api.exitMaximizedGroup();
      return;
    }
    const active = api.activePanel;
    if (active) api.maximizeGroup(active);
  }, []);

  // Live language switch: refresh the titles dockview keeps for each panel.
  const language = useLanguage();
  useEffect(() => {
    for (const panel of apiRef.current?.panels ?? []) {
      const def = getPanel(panel.id);
      if (def && panel.title !== def.title) panel.api.setTitle(def.title);
    }
  }, [language]);

  useEffect(() => {
    registerWorkspace({ openPanel, resetLayout, toggleMaximize });
    return () => registerWorkspace(null);
  }, [openPanel, resetLayout, toggleMaximize]);

  const onReady = (event: DockviewReadyEvent) => {
    apiRef.current = event.api;
    currentApi = event.api;
    let restored = false;
    try {
      const raw = window.localStorage.getItem(LAYOUT_KEY);
      const parsed: unknown = raw ? JSON.parse(raw) : null;
      if (layoutIsValid(parsed)) {
        event.api.fromJSON(parsed as Parameters<DockviewApi["fromJSON"]>[0]);
        restored = true;
      }
    } catch (e) {
      console.warn("ignoring invalid saved layout", e);
    }
    if (!restored) applyPreset(event.api);
    event.api.onDidLayoutChange(() => {
      if (saveTimer.current) clearTimeout(saveTimer.current);
      saveTimer.current = setTimeout(() => {
        try {
          window.localStorage.setItem(LAYOUT_KEY, JSON.stringify(event.api.toJSON()));
        } catch {
          /* storage unavailable */
        }
      }, 400);
    });
  };

  return (
    <div className="workspace">
      <DockviewReact
        theme={NEBULA_DOCK_THEME}
        components={COMPONENTS}
        tabComponents={{ ok: PanelTab }}
        defaultTabComponent={PanelTab}
        watermarkComponent={Watermark}
        messages={DOCK_MESSAGES}
        onReady={onReady}
      />
    </div>
  );
}
