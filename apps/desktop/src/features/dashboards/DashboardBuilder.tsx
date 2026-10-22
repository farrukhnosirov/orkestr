/**
 * Dashboard Builder (ADR-0034): global / project scope, dashboard list
 * (create, rename, delete), 12-column grid with pointer + keyboard drag and
 * resize, widget palette, schema-generated config forms, view mode. Every
 * committed edit is persisted with `dashboards.save` (optimistic concurrency
 * via `expectedVersion`).
 */
import type { Dashboard, WidgetDescriptor, WidgetInstance } from "@orkestr/protocol";
import { topic, TOPIC_GLOBAL } from "@orkestr/protocol";
import { Badge, Button, EmptyState, IconButton, Input, Tabs } from "@orkestr/ui";
import { clsx } from "clsx";
import { Check, GripVertical, LayoutGrid, Pencil, Plus, Puzzle, Scaling, Settings2, Trash2, X } from "lucide-react";
import { type FormEvent, type KeyboardEvent, type PointerEvent, useCallback, useEffect, useMemo, useRef, useState } from "react";

import { getClient } from "../../core/client";
import { contextMenuHandler } from "../contextMenu";
import { useT } from "../../i18n";
import { errorMessage, useRpcQuery } from "../../lib/rpcQuery";
import { useAppStore } from "../../store/app";
import { BuiltinProvider, BuiltinWidget, builtinIcon, builtinTitle, isBuiltin, needsMissionContext } from "./builtins";
import { ConfigForm } from "./ConfigForm";
import { defaultConfig } from "./configSchema";
import { addItem, GRID_COLUMNS, moveItem, newInstanceId, nextWidthPreset, removeItem, resizeItem, rowCount } from "./grid";
import { WidgetFrame } from "./WidgetFrame";
import "./dashboards.css";

/** Row height of the grid in px (gap excluded). */
export const ROW_PX = 64;
const GAP_PX = 12;

export type DashboardScope = "global" | "project";

interface DragState {
  id: string;
  mode: "move" | "resize";
  startX: number;
  startY: number;
  origin: WidgetInstance;
  colPx: number;
  rowPx: number;
  preview: WidgetInstance;
}

function cellStyle(i: Pick<WidgetInstance, "x" | "y" | "w" | "h">) {
  return { gridColumn: `${i.x + 1} / span ${i.w}`, gridRow: `${i.y + 1} / span ${i.h}` };
}

function newer(a: Dashboard | null, b: Dashboard | null | undefined): Dashboard | null {
  if (!b) return a;
  if (!a || a.id !== b.id) return b;
  return b.version > a.version ? b : a;
}

export function DashboardBuilder() {
  const { t } = useT("dashboards");
  const activeProjectId = useAppStore((s) => s.activeProjectId);
  const [scope, setScope] = useState<DashboardScope>("global");
  const projectId = scope === "project" ? activeProjectId : null;
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [editing, setEditing] = useState(false);
  const [naming, setNaming] = useState<{ mode: "create" | "rename"; value: string } | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [configuring, setConfiguring] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [local, setLocal] = useState<Dashboard | null>(null);
  const [drag, setDrag] = useState<DragState | null>(null);
  const gridRef = useRef<HTMLDivElement>(null);
  const saving = useRef<Promise<unknown>>(Promise.resolve());

  const scopeReady = scope === "global" || projectId !== null;
  const topics = useMemo(() => (projectId ? [TOPIC_GLOBAL, topic.project(projectId)] : [TOPIC_GLOBAL]), [projectId]);

  const list = useRpcQuery(
    `dashboards.list:${scope}:${projectId ?? ""}`,
    async (c) =>
      (await c.call("dashboards.list", projectId ? { projectId, includeGlobal: false } : { includeGlobal: true })).dashboards,
    { topics, reloadOn: ["DashboardChanged"], enabled: scopeReady },
  );
  const catalog = useRpcQuery("widgets.list", async (c) => (await c.call("widgets.list", {})).widgets, {
    topics: [TOPIC_GLOBAL],
    reloadOn: ["PluginStateChanged", "PluginInstalled"],
  });
  const byId = useMemo(() => new Map((catalog.data ?? []).map((d) => [d.id, d])), [catalog.data]);

  const summaries = scopeReady ? (list.data ?? []) : [];
  const effectiveId = selectedId && summaries.some((s) => s.id === selectedId) ? selectedId : (summaries[0]?.id ?? null);
  const remote = useRpcQuery(
    `dashboards.get:${effectiveId ?? ""}`,
    async (c) => (await c.call("dashboards.get", { dashboardId: effectiveId! })).dashboard,
    { topics, reloadOn: ["DashboardChanged"], enabled: effectiveId !== null },
  );
  const dashboard = effectiveId ? newer(local?.id === effectiveId ? local : null, remote.data?.id === effectiveId ? remote.data : null) : null;
  const items = dashboard?.widgets ?? [];

  useEffect(() => {
    setConfirmDelete(false);
    setConfiguring(null);
  }, [effectiveId]);

  /** Persists a new layout / name (serialized; conflicts reload). */
  const persist = useCallback(
    (next: { widgets?: WidgetInstance[]; name?: string }) => {
      if (!dashboard) return Promise.resolve();
      const optimistic: Dashboard = { ...dashboard, widgets: next.widgets ?? dashboard.widgets, name: next.name ?? dashboard.name };
      setLocal(optimistic);
      const run = saving.current.then(async () => {
        try {
          const res = await getClient().call("dashboards.save", {
            dashboardId: dashboard.id,
            name: optimistic.name,
            widgets: optimistic.widgets,
            expectedVersion: dashboard.version,
          });
          setLocal(res.dashboard);
          setError(null);
        } catch (e) {
          setError(errorMessage(e));
          setLocal(null);
          remote.reload();
        }
      });
      saving.current = run;
      return run;
    },
    [dashboard, remote],
  );

  const create = async (name: string) => {
    try {
      const res = await getClient().call("dashboards.save", {
        ...(projectId ? { projectId } : {}),
        name,
        widgets: [],
      });
      setLocal(res.dashboard);
      setSelectedId(res.dashboard.id);
      setEditing(true);
      setError(null);
      list.reload();
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  const remove = async () => {
    if (!dashboard) return;
    try {
      await getClient().call("dashboards.delete", { dashboardId: dashboard.id });
      setSelectedId(null);
      setLocal(null);
      setConfirmDelete(false);
      list.reload();
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  const submitName = (e: FormEvent) => {
    e.preventDefault();
    if (!naming) return;
    const name = naming.value.trim();
    if (!name) return;
    if (naming.mode === "create") void create(name);
    else void persist({ name });
    setNaming(null);
  };

  const addWidget = (d: WidgetDescriptor) => {
    const size = d.sizes[0] ?? { w: 4, h: 3 };
    void persist({
      widgets: addItem(items, {
        instanceId: newInstanceId(items),
        widgetId: d.id,
        w: size.w,
        h: size.h,
        config: defaultConfig(d.configSchema),
      }),
    });
  };

  // ── pointer drag / resize ───────────────────────────────────────────
  const startDrag = (e: PointerEvent<HTMLElement>, item: WidgetInstance, mode: DragState["mode"]) => {
    if (!editing || e.button !== 0) return;
    const rect = gridRef.current?.getBoundingClientRect();
    if (!rect || rect.width <= 0) return;
    e.preventDefault();
    (e.currentTarget as HTMLElement).setPointerCapture?.(e.pointerId);
    setDrag({
      id: item.instanceId,
      mode,
      startX: e.clientX,
      startY: e.clientY,
      origin: item,
      colPx: (rect.width + GAP_PX) / GRID_COLUMNS,
      rowPx: ROW_PX + GAP_PX,
      preview: item,
    });
  };
  const onDragMove = (e: PointerEvent<HTMLElement>) => {
    if (!drag) return;
    const dx = Math.round((e.clientX - drag.startX) / drag.colPx);
    const dy = Math.round((e.clientY - drag.startY) / drag.rowPx);
    const o = drag.origin;
    const preview =
      drag.mode === "move"
        ? { ...o, x: Math.max(0, Math.min(GRID_COLUMNS - o.w, o.x + dx)), y: Math.max(0, o.y + dy) }
        : { ...o, w: Math.max(1, Math.min(GRID_COLUMNS - o.x, o.w + dx)), h: Math.max(1, Math.min(24, o.h + dy)) };
    if (preview.x !== drag.preview.x || preview.y !== drag.preview.y || preview.w !== drag.preview.w || preview.h !== drag.preview.h) {
      setDrag({ ...drag, preview });
    }
  };
  const endDrag = () => {
    if (!drag) return;
    const p = drag.preview;
    const o = drag.origin;
    setDrag(null);
    if (p.x === o.x && p.y === o.y && p.w === o.w && p.h === o.h) return;
    void persist({ widgets: drag.mode === "move" ? moveItem(items, p.instanceId, p.x, p.y) : resizeItem(items, p.instanceId, p.w, p.h) });
  };

  const onGripKey = (e: KeyboardEvent<HTMLElement>, item: WidgetInstance) => {
    const delta: Record<string, [number, number]> = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1] };
    const d = delta[e.key];
    if (!d || !editing) return;
    e.preventDefault();
    const [dx, dy] = d;
    void persist({
      widgets: e.shiftKey
        ? resizeItem(items, item.instanceId, item.w + dx, item.h + dy)
        : moveItem(items, item.instanceId, item.x + dx, Math.max(0, item.y + dy)),
    });
  };

  const rendered = drag ? items.map((i) => (i.instanceId === drag.id ? drag.preview : i)) : items;
  const rows = Math.max(rowCount(rendered) + (editing ? 2 : 0), 4);
  const hasBuiltin = items.some((i) => needsMissionContext(i.widgetId));
  const configTarget = configuring ? items.find((i) => i.instanceId === configuring) : undefined;
  const configDesc = configTarget ? byId.get(configTarget.widgetId) : undefined;

  const labelOf = (d: WidgetDescriptor) =>
    isBuiltin(d.id) ? builtinTitle(d.id, d.id === "builtin:note" ? t("builtin.note") : d.title) : d.title;
  const titleOf = (i: WidgetInstance, d: WidgetDescriptor | undefined) => i.title ?? (d ? labelOf(d) : i.widgetId);

  const grid = (
    <div
      ref={gridRef}
      className={clsx("dsh-grid", editing && "dsh-grid--editing")}
      style={{ gridTemplateRows: `repeat(${rows}, ${ROW_PX}px)` }}
      onPointerMove={onDragMove}
      onPointerUp={endDrag}
      onPointerCancel={() => setDrag(null)}
      data-testid="dashboard-grid"
    >
      {rendered.map((item) => {
        const d = byId.get(item.widgetId);
        const Icon = isBuiltin(item.widgetId) ? builtinIcon(item.widgetId) : Puzzle;
        const title = titleOf(item, d);
        return (
          <section
            key={item.instanceId}
            className={clsx("mc-widget dsh-cell", drag?.id === item.instanceId && "dsh-cell--dragging")}
            style={cellStyle(item)}
            aria-label={title}
            data-instance={item.instanceId}
          >
            <header
              className="mc-widget__head"
              onContextMenu={contextMenuHandler(() => [
                { id: "widget.configure", label: t("grid.configure"), icon: <Settings2 size={14} />, onSelect: () => setConfiguring(item.instanceId) },
                {
                  id: "widget.width",
                  label: t("grid.widthPreset"),
                  icon: <Scaling size={14} />,
                  onSelect: () => void persist({ widgets: resizeItem(items, item.instanceId, nextWidthPreset(item.w), item.h) }),
                },
                { kind: "separator", id: "widget.s1" },
                { id: "widget.remove", label: t("grid.remove"), icon: <X size={14} />, danger: true, onSelect: () => void persist({ widgets: removeItem(items, item.instanceId) }) },
              ])}
            >
              {editing && (
                <span
                  className="mc-widget__grip dsh-grip"
                  role="button"
                  tabIndex={0}
                  aria-label={t("grid.dragHandle", { title })}
                  title={t("grid.dragHint")}
                  onPointerDown={(e) => startDrag(e, item, "move")}
                  onKeyDown={(e) => onGripKey(e, item)}
                >
                  <GripVertical size={13} />
                </span>
              )}
              {Icon && <Icon size={14} className="mc-widget__icon" aria-hidden="true" />}
              <h3 className="mc-widget__title">{title}</h3>
              {d && !isBuiltin(d.id) && <Badge tone="info">{t("palette.plugin")}</Badge>}
              {editing && (
                <span className="mc-widget__tools">
                  <IconButton size="sm" label={t("grid.configure")} icon={<Settings2 size={13} />} onClick={() => setConfiguring(item.instanceId)} />
                  <IconButton
                    size="sm"
                    label={t("grid.widthPreset")}
                    icon={<Scaling size={13} />}
                    onClick={() => void persist({ widgets: resizeItem(items, item.instanceId, nextWidthPreset(item.w), item.h) })}
                  />
                  <IconButton
                    size="sm"
                    label={t("grid.remove")}
                    icon={<X size={13} />}
                    onClick={() => void persist({ widgets: removeItem(items, item.instanceId) })}
                  />
                </span>
              )}
            </header>
            <div className="mc-widget__body dsh-cell__body">
              {!d ? (
                <div className="mc-error" role="alert">
                  {t("widget.unavailable", { id: item.widgetId })}
                </div>
              ) : isBuiltin(d.id) ? (
                <BuiltinWidget widgetId={d.id} w={item.w} config={item.config} />
              ) : (
                <WidgetFrame
                  descriptor={d}
                  instanceId={item.instanceId}
                  config={item.config}
                  topics={topics}
                  onResize={editing ? (s) => void persist({ widgets: resizeItem(items, item.instanceId, s.w, s.h) }) : undefined}
                />
              )}
            </div>
            {editing && (
              <span
                className="dsh-resize"
                role="button"
                tabIndex={-1}
                aria-label={t("grid.resizeHandle", { title })}
                onPointerDown={(e) => startDrag(e, item, "resize")}
              />
            )}
          </section>
        );
      })}
    </div>
  );

  return (
    <div className="dsh" data-editing={editing ? "on" : "off"}>
      <div className="dsh-toolbar">
        <Tabs<DashboardScope>
          aria-label={t("scope.label")}
          items={[
            { id: "global", label: t("scope.global") },
            { id: "project", label: t("scope.project"), disabled: !activeProjectId },
          ]}
          value={scope}
          onChange={(s) => {
            setScope(s);
            setSelectedId(null);
            setLocal(null);
          }}
        />
        <select
          className="dsh-select"
          aria-label={t("list.label")}
          value={effectiveId ?? ""}
          disabled={summaries.length === 0}
          onChange={(e) => setSelectedId(e.target.value || null)}
        >
          {summaries.map((s) => (
            <option key={s.id} value={s.id}>
              {s.name}
            </option>
          ))}
        </select>
        <span className="dsh-toolbar__spacer" />
        <Button size="sm" leftIcon={<Plus size={14} />} disabled={!scopeReady} onClick={() => setNaming({ mode: "create", value: "" })}>
          {t("actions.create")}
        </Button>
        {dashboard && (
          <>
            <IconButton size="sm" label={t("actions.rename")} icon={<Pencil size={14} />} onClick={() => setNaming({ mode: "rename", value: dashboard.name })} />
            {confirmDelete ? (
              <Button size="sm" variant="danger" leftIcon={<Trash2 size={14} />} onClick={() => void remove()}>
                {t("actions.confirmDelete")}
              </Button>
            ) : (
              <IconButton size="sm" label={t("actions.delete")} icon={<Trash2 size={14} />} onClick={() => setConfirmDelete(true)} />
            )}
            <Button
              size="sm"
              variant={editing ? "primary" : "secondary"}
              leftIcon={editing ? <Check size={14} /> : <LayoutGrid size={14} />}
              onClick={() => setEditing((v) => !v)}
            >
              {editing ? t("actions.done") : t("actions.edit")}
            </Button>
          </>
        )}
      </div>

      {naming && (
        <form className="dsh-naming" onSubmit={submitName}>
          <Input
            autoFocus
            label={naming.mode === "create" ? t("naming.createLabel") : t("naming.renameLabel")}
            value={naming.value}
            maxLength={120}
            onChange={(e) => setNaming({ ...naming, value: e.target.value })}
          />
          <Button type="submit" variant="primary" size="sm" disabled={!naming.value.trim()}>
            {naming.mode === "create" ? t("actions.create") : t("actions.save")}
          </Button>
          <Button type="button" variant="ghost" size="sm" onClick={() => setNaming(null)}>
            {t("actions.cancel")}
          </Button>
        </form>
      )}

      {error && (
        <div className="mc-error dsh-error" role="alert">
          {error}
        </div>
      )}

      {!scopeReady ? (
        <EmptyState icon={<LayoutGrid size={28} strokeWidth={1.6} />} title={t("empty.noProjectTitle")} description={t("empty.noProject")} />
      ) : !dashboard ? (
        <EmptyState
          icon={<LayoutGrid size={28} strokeWidth={1.6} />}
          title={t("empty.title")}
          description={list.loading ? t("widget.loading") : t("empty.description")}
          action={
            <Button variant="primary" leftIcon={<Plus size={14} />} onClick={() => setNaming({ mode: "create", value: "" })}>
              {t("actions.create")}
            </Button>
          }
        />
      ) : (
        <div className="dsh-body">
          <div className="mc dsh-board">
            {items.length === 0 && !editing ? (
              <EmptyState icon={<LayoutGrid size={28} strokeWidth={1.6} />} title={t("empty.noWidgetsTitle")} description={t("empty.noWidgets")} />
            ) : hasBuiltin ? (
              <BuiltinProvider>{grid}</BuiltinProvider>
            ) : (
              grid
            )}
          </div>
          {editing && (
            <aside className="dsh-palette" aria-label={t("palette.title")}>
              <h3 className="dsh-palette__title">{t("palette.title")}</h3>
              {catalog.error && <div className="mc-error">{catalog.error}</div>}
              <ul>
                {(catalog.data ?? []).map((d) => {
                  const Icon = isBuiltin(d.id) ? builtinIcon(d.id) : Puzzle;
                  const label = labelOf(d);
                  return (
                    <li key={d.id}>
                      <button type="button" className="dsh-palette__item" onClick={() => addWidget(d)} aria-label={t("palette.add", { title: label })}>
                        {Icon && <Icon size={14} aria-hidden="true" />}
                        <span>{label}</span>
                        {!isBuiltin(d.id) && <Badge tone="info">{t("palette.plugin")}</Badge>}
                      </button>
                    </li>
                  );
                })}
              </ul>
            </aside>
          )}
        </div>
      )}

      {configTarget && (
        <div className="dsh-dialog" role="dialog" aria-modal="true" aria-label={t("config.title")}>
          <div className="dsh-dialog__card">
            <h3>{t("config.title")}</h3>
            {configDesc ? (
              <ConfigForm
                schema={configDesc.configSchema}
                value={configTarget.config}
                title={configTarget.title}
                onCancel={() => setConfiguring(null)}
                onSubmit={(config, title) => {
                  setConfiguring(null);
                  void persist({
                    widgets: items.map((i) => {
                      if (i.instanceId !== configTarget.instanceId) return i;
                      const { title: _old, ...rest } = i;
                      return title ? { ...rest, config, title } : { ...rest, config };
                    }),
                  });
                }}
              />
            ) : (
              <div className="mc-error">{t("widget.unavailable", { id: configTarget.widgetId })}</div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
