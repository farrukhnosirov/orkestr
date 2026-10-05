import { Badge, Button, IconButton, StatusDot } from "@orkestr/ui";
import { clsx } from "clsx";
import { Eye, EyeOff, GripVertical, LayoutGrid, Maximize2, Minimize2, MoveLeft, MoveRight, RefreshCw, RotateCcw, Scaling } from "lucide-react";
import { motion } from "motion/react";
import { type KeyboardEvent, useMemo, useState } from "react";

import { useFormat, useT } from "../../i18n";
import { useAppStore } from "../../store/app";
import { SIZE_COLUMNS, type WidgetPlacement } from "./arrangement";
import { MissionContext, type MissionContextValue } from "./context";
import { useDashboardSnapshot } from "./data";
import { useMissionControlStore } from "./store";
import { useVisualBudget } from "./visual";
import { getWidget } from "./widgets/registry";

const DRAG_TYPE = "application/x-orkestr-widget";

function WidgetFrame({ placement, dragOver, onDragOverId }: { placement: WidgetPlacement; dragOver: boolean; onDragOverId: (id: string | null) => void }) {
  const { t } = useT("missionControl");
  const def = getWidget(placement.id);
  const editing = useMissionControlStore((s) => s.editing);
  const { move, moveBy, cycle, hide } = useMissionControlStore.getState();
  const budget = useVisualBudget();
  if (!def) return null;
  const Icon = def.icon;
  const Component = def.component;
  const onKey = (e: KeyboardEvent<HTMLElement>) => {
    if (!e.altKey || !e.shiftKey) return;
    if (e.key === "ArrowLeft" || e.key === "ArrowUp") {
      e.preventDefault();
      moveBy(def.id, -1);
    } else if (e.key === "ArrowRight" || e.key === "ArrowDown") {
      e.preventDefault();
      moveBy(def.id, 1);
    }
  };
  return (
    <motion.section
      layout={budget.animate ? "position" : false}
      transition={{ type: "spring", stiffness: 420, damping: 38 }}
      className={clsx("mc-widget", `mc-widget--${placement.size}`, dragOver && "mc-widget--drop", editing && "mc-widget--editing")}
      style={{ ["--mc-span" as string]: SIZE_COLUMNS[placement.size] }}
      aria-label={def.title}
      data-widget={def.id}
      onDragOver={(e) => {
        if (!e.dataTransfer.types.includes(DRAG_TYPE)) return;
        e.preventDefault();
        e.dataTransfer.dropEffect = "move";
        onDragOverId(def.id);
      }}
      onDragLeave={() => onDragOverId(null)}
      onDrop={(e) => {
        const id = e.dataTransfer.getData(DRAG_TYPE);
        onDragOverId(null);
        if (id) {
          e.preventDefault();
          move(id, def.id);
        }
      }}
    >
      <header className="mc-widget__head" onKeyDown={onKey}>
        <span
          className="mc-widget__grip"
          draggable
          tabIndex={0}
          role="button"
          aria-label={t("layout.dragHandle", { title: def.title })}
          title={t("layout.dragHint")}
          onDragStart={(e) => {
            e.dataTransfer.setData(DRAG_TYPE, def.id);
            e.dataTransfer.effectAllowed = "move";
            const section = (e.currentTarget as HTMLElement).closest("section");
            if (section) e.dataTransfer.setDragImage(section, 24, 18);
          }}
        >
          <GripVertical size={13} />
        </span>
        <Icon size={14} className="mc-widget__icon" aria-hidden="true" />
        <h3 className="mc-widget__title">{def.title}</h3>
        {editing && (
          <span className="mc-widget__tools">
            <IconButton size="sm" label={t("layout.moveLeft")} icon={<MoveLeft size={13} />} onClick={() => moveBy(def.id, -1)} />
            <IconButton size="sm" label={t("layout.moveRight")} icon={<MoveRight size={13} />} onClick={() => moveBy(def.id, 1)} />
            {def.sizes.length > 1 && (
              <IconButton size="sm" label={t("layout.resize", { size: t(`layout.size.${placement.size}`) })} icon={<Scaling size={13} />} onClick={() => cycle(def.id)} />
            )}
            <IconButton size="sm" label={t("layout.hide")} icon={<EyeOff size={13} />} onClick={() => hide(def.id, true)} />
          </span>
        )}
      </header>
      <div className="mc-widget__body">
        <Component size={placement.size} />
      </div>
    </motion.section>
  );
}

/** The Mission Control dashboard (panel body and full-screen view). */
export function MissionDashboard({ variant }: { variant: "panel" | "fullscreen" }) {
  const { t } = useT("missionControl");
  const fmt = useFormat();
  const snapshot = useDashboardSnapshot();
  const budget = useVisualBudget();
  const activeProjectId = useAppStore((s) => s.activeProjectId);
  const [pickedProject, setPickedProject] = useState<string | null>(null);
  const [dragOver, setDragOver] = useState<string | null>(null);
  const arrangement = useMissionControlStore((s) => s.arrangement);
  const editing = useMissionControlStore((s) => s.editing);
  const fullscreen = useMissionControlStore((s) => s.fullscreen);
  const { setEditing, toggleFullscreen, reset, hide } = useMissionControlStore.getState();

  const projects = snapshot.data?.projects;
  const focusProjectId =
    (pickedProject && projects?.some((p) => p.projectId === pickedProject) ? pickedProject : null) ??
    activeProjectId ??
    projects?.[0]?.projectId ??
    null;

  const ctx = useMemo<MissionContextValue>(
    () => ({ snapshot, budget, focusProjectId, setFocusProjectId: setPickedProject }),
    [snapshot, budget, focusProjectId],
  );
  const visible = arrangement.widgets.filter((w) => !w.hidden && getWidget(w.id));
  const hidden = arrangement.widgets.filter((w) => w.hidden && getWidget(w.id));
  const live = snapshot.data && !snapshot.error;

  return (
    <MissionContext.Provider value={ctx}>
      <div
        className={clsx("mc", `mc--${variant}`, budget.rich && "mc--rich", !budget.animate && "mc--still")}
        data-mc-animate={budget.animate ? "on" : "off"}
      >
        <div className="mc-toolbar">
          <div className="mc-toolbar__title">
            <LayoutGrid size={16} aria-hidden="true" />
            <h2>{t("title")}</h2>
            <span className="mc-live">
              <StatusDot status={live ? "online" : snapshot.error ? "error" : "connecting"} pulse={!!live && budget.animate} />
              {live && snapshot.data
                ? t("toolbar.updated", { time: fmt.date(snapshot.data.generatedAt, "timeSeconds") })
                : snapshot.error
                  ? t("toolbar.offline")
                  : t("toolbar.connecting")}
            </span>
          </div>
          <div className="mc-toolbar__actions">
            {editing && (
              <Button size="sm" variant="ghost" onClick={reset} leftIcon={<RotateCcw size={13} />}>
                {t("layout.reset")}
              </Button>
            )}
            <Button size="sm" variant={editing ? "primary" : "secondary"} onClick={() => setEditing(!editing)} aria-pressed={editing}>
              {editing ? t("layout.done") : t("layout.customize")}
            </Button>
            <IconButton label={t("toolbar.refresh")} icon={<RefreshCw size={14} />} onClick={snapshot.reload} />
            <IconButton
              label={fullscreen ? t("toolbar.exitFullscreen") : t("toolbar.fullscreen")}
              tooltip
              icon={fullscreen ? <Minimize2 size={14} /> : <Maximize2 size={14} />}
              onClick={toggleFullscreen}
            />
          </div>
        </div>
        {editing && hidden.length > 0 && (
          <div className="mc-hidden" aria-label={t("layout.hiddenWidgets")}>
            <span>{t("layout.hiddenWidgets")}</span>
            {hidden.map((w) => {
              const def = getWidget(w.id);
              return (
                <button key={w.id} type="button" className="mc-chip" onClick={() => hide(w.id, false)}>
                  <Eye size={12} aria-hidden="true" />
                  {t("layout.show", { title: def?.title ?? w.id })}
                </button>
              );
            })}
          </div>
        )}
        {editing && (
          <p className="mc-hint">
            <Badge tone="violet">{t("layout.editing")}</Badge> {t("layout.editHint")}
          </p>
        )}
        <div className="mc-grid">
          {visible.map((w) => (
            <WidgetFrame key={w.id} placement={w} dragOver={dragOver === w.id} onDragOverId={setDragOver} />
          ))}
        </div>
      </div>
    </MissionContext.Provider>
  );
}
