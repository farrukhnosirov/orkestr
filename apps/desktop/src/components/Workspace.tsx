import { Button, EmptyState } from "@orkestr/ui";
import { clsx } from "clsx";
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
import { type FunctionComponent, useEffect, useRef } from "react";

import * as layout from "../features/layout/controller";
import { GroupActions } from "../features/layout/GroupActions";
import { LayoutEditorCanvas } from "../features/layout/LayoutEditor";
import { useLayoutStore } from "../features/layout/store";
import { t as translate, useLanguage, useT } from "../i18n";
import { getPanel, PANELS } from "../panels/registry";
import { useAppStore } from "../store/app";
import { registerWorkspace } from "./workspaceActions";

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

/** Tab with the panel icon and a close button (hidden while the layout is locked). */
function PanelTab({ api, params }: IDockviewPanelHeaderProps) {
  const def = getPanel((params as { panelId?: string } | undefined)?.panelId ?? api.id);
  const Icon = def?.icon;
  const locked = useLayoutStore((s) => s.locked);
  const { t } = useT("shell");
  // Registry titles are localized getters: prefer them over the title saved in the layout.
  const title = def?.title ?? api.title ?? api.id;
  return (
    <div className="dock-tab" onMouseDown={(e) => !locked && e.button === 1 && api.close()}>
      {Icon && <Icon size={13} strokeWidth={2} className="dock-tab__icon" />}
      <span className="dock-tab__title">{title}</span>
      {!locked && (
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
      )}
    </div>
  );
}

function Watermark(_props: IWatermarkPanelProps) {
  const { t } = useT("shell");
  const popouts = useLayoutStore((s) => s.popouts.length);
  return (
    <EmptyState
      icon={<LayoutGrid size={26} strokeWidth={1.6} />}
      title={t("workspace.emptyTitle")}
      description={popouts > 0 ? translate("layout:workspace.allDetached", { count: popouts }) : t("workspace.emptyDescription")}
      action={
        <Button variant="primary" onClick={() => void layout.resetActive()}>
          {translate("layout:workspace.restore")}
        </Button>
      }
    />
  );
}

/** The docking workspace (§14, §28). Layout logic lives in `features/layout/controller`. */
export function Workspace() {
  const apiRef = useRef<DockviewApi | null>(null);
  const editing = useLayoutStore((s) => s.editorOpen);
  const locked = useLayoutStore((s) => s.locked);

  // Live language switch: refresh the titles dockview keeps for each panel.
  const language = useLanguage();
  useEffect(() => {
    for (const panel of apiRef.current?.panels ?? []) {
      const def = getPanel(panel.id);
      if (def && panel.title !== def.title) panel.api.setTitle(def.title);
    }
  }, [language]);

  useEffect(() => {
    registerWorkspace({
      openPanel: layout.openPanel,
      resetLayout: () => void layout.resetActive(),
      toggleMaximize: () => layout.toggleMaximize(),
    });
    return () => {
      registerWorkspace(null);
      layout.detachDock();
    };
  }, []);

  const onReady = (event: DockviewReadyEvent) => {
    apiRef.current = event.api;
    void layout.attachDock(event.api);
  };

  return (
    <div className={clsx("workspace", editing && "workspace--editing", locked && "workspace--locked")}>
      <DockviewReact
        theme={NEBULA_DOCK_THEME}
        components={COMPONENTS}
        tabComponents={{ ok: PanelTab }}
        defaultTabComponent={PanelTab}
        rightHeaderActionsComponent={GroupActions}
        watermarkComponent={Watermark}
        messages={DOCK_MESSAGES}
        onReady={onReady}
      />
      {editing && <LayoutEditorCanvas />}
    </div>
  );
}
