/**
 * Layout Editor (§29): a glass side sheet with presets, the panel picker,
 * the lock toggle and the multi-monitor map, plus a canvas overlay inside the
 * workspace that highlights groups and drop zones while editing.
 */
import "./layout.css";

import { Badge, Button } from "@orkestr/ui";
import { clsx } from "clsx";
import {
  AppWindow,
  Download,
  Eye,
  EyeOff,
  LayoutTemplate,
  Lock,
  MonitorSmartphone,
  Pencil,
  PictureInPicture2,
  RotateCcw,
  Save,
  SquareArrowDownLeft,
  Star,
  Trash2,
  Unlock,
  Upload,
  X,
} from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useMemo } from "react";

import { useT } from "../../i18n";
import { PANELS } from "../../panels/registry";
import * as layout from "./controller";
import { useCalmMotion } from "./motion";
import { layoutDescription, layoutName } from "./names";
import { BUILTIN_PRESETS } from "./presets";
import { layoutEntries, monitorMapGeometry, monitorNumber, type MonitorInfo, type WindowInfo } from "./selection";
import { useLayoutStore } from "./store";
import { canDetach } from "./windowsApi";

/** Grid + drop-zone hint drawn over the dock while editing (pointer-transparent). */
export function LayoutEditorCanvas() {
  const { t } = useT("layout");
  return (
    <div className="layout-canvas" aria-hidden="true">
      <div className="layout-canvas__grid" />
      <div className="layout-canvas__hint">
        <LayoutTemplate size={13} />
        <span>{t("editor.canvasHint")}</span>
      </div>
    </div>
  );
}

function PresetsSection() {
  const { t } = useT("layout");
  const state = useLayoutStore((s) => s.state);
  const activeId = useLayoutStore((s) => s.activeId);
  const persisted = useLayoutStore((s) => s.loaded);
  const entries = useMemo(() => layoutEntries(state), [state]);
  const user = entries.filter((e) => !e.builtin);
  const customised = entries.find((e) => e.id === activeId)?.customised ?? false;
  return (
    <section className="lyt-section">
      <h3 className="lyt-section__title">{t("editor.sections.presets")}</h3>
      <div className="lyt-presets">
        {BUILTIN_PRESETS.map((p) => {
          const Icon = p.icon;
          const entry = entries.find((e) => e.id === p.id);
          return (
            <button
              key={p.id}
              type="button"
              className={clsx("lyt-preset", activeId === p.id && "lyt-preset--active")}
              onClick={() => void layout.switchLayout(p.id)}
              title={layoutDescription(p.id) ?? undefined}
            >
              <span className="lyt-preset__icon">
                <Icon size={16} />
                {entry?.customised && <span className="lyt-dot" title={t("switcher.customised")} />}
              </span>
              <span className="lyt-preset__name">{layoutName(p.id)}</span>
              {p.shortcut && <kbd className="lyt-kbd lyt-preset__kbd">{p.shortcut}</kbd>}
            </button>
          );
        })}
      </div>
      {user.length > 0 && (
        <ul className="lyt-list">
          {user.map((e) => (
            <li key={e.id} className={clsx("lyt-row", activeId === e.id && "lyt-row--active")}>
              <button type="button" className="lyt-row__main" onClick={() => void layout.switchLayout(e.id)}>
                <LayoutTemplate size={14} />
                <span className="lyt-row__name">{e.name}</span>
                {e.popouts > 0 && <Badge tone="violet">{t("switcher.windows", { count: e.popouts })}</Badge>}
              </button>
              <button
                type="button"
                className="lyt-icon-btn"
                title={t("editor.rename")}
                aria-label={t("editor.rename")}
                onClick={() => useLayoutStore.getState().patch({ dialog: { kind: "rename", id: e.id, name: e.name } })}
              >
                <Pencil size={13} />
              </button>
              <button
                type="button"
                className="lyt-icon-btn lyt-icon-btn--danger"
                title={t("editor.delete")}
                aria-label={t("editor.delete")}
                onClick={() => {
                  if (window.confirm(t("dialogs.deleteConfirm", { name: e.name }))) void layout.deleteLayout(e.id);
                }}
              >
                <Trash2 size={13} />
              </button>
            </li>
          ))}
        </ul>
      )}
      <div className="lyt-actions">
        <Button size="sm" variant="primary" leftIcon={<Save size={13} />} disabled={!persisted} onClick={() => useLayoutStore.getState().patch({ dialog: { kind: "saveAs" } })}>
          {t("editor.saveAs")}
        </Button>
        <Button size="sm" variant="ghost" leftIcon={<RotateCcw size={13} />} onClick={() => void layout.resetActive()} title={customised ? undefined : t("editor.resetHint")}>
          {t("editor.reset")}
        </Button>
        <Button size="sm" variant="ghost" leftIcon={<Download size={13} />} onClick={() => void layout.exportCurrent()}>
          {t("editor.export")}
        </Button>
        <Button size="sm" variant="ghost" leftIcon={<Upload size={13} />} disabled={!persisted} onClick={() => useLayoutStore.getState().patch({ dialog: { kind: "import" } })}>
          {t("editor.import")}
        </Button>
      </div>
    </section>
  );
}

type PanelPlace = { kind: "dock" } | { kind: "floating" } | { kind: "window"; monitor: number | null } | { kind: "hidden" };

function placeOf(id: string, dock: string[], floating: string[], popouts: WindowInfo[], monitors: MonitorInfo[]): PanelPlace {
  const w = popouts.find((p) => p.panelId === id);
  if (w) return { kind: "window", monitor: monitorNumber(monitors, w.monitor) };
  if (floating.includes(id)) return { kind: "floating" };
  if (dock.includes(id)) return { kind: "dock" };
  return { kind: "hidden" };
}

function PanelsSection() {
  const { t } = useT("layout");
  const dock = useLayoutStore((s) => s.dockPanels);
  const floating = useLayoutStore((s) => s.floatingPanels);
  const popouts = useLayoutStore((s) => s.popouts);
  const monitors = useLayoutStore((s) => s.monitors);
  const locked = useLayoutStore((s) => s.locked);
  const detachable = canDetach();
  return (
    <section className="lyt-section">
      <h3 className="lyt-section__title">{t("editor.sections.panels")}</h3>
      <ul className="lyt-list">
        {PANELS.map((p) => {
          const Icon = p.icon;
          const place = placeOf(p.id, dock, floating, popouts, monitors);
          const shown = place.kind !== "hidden";
          const status =
            place.kind === "window"
              ? place.monitor !== null
                ? t("editor.place.windowOn", { n: place.monitor })
                : t("editor.place.window")
              : t(`editor.place.${place.kind}`);
          const target = place.kind === "window" ? (popouts.find((w) => w.panelId === p.id)?.monitor ?? "dock") : "dock";
          return (
            <li key={p.id} className={clsx("lyt-row", !shown && "lyt-row--muted")}>
              <span className="lyt-row__main lyt-row__main--static">
                <Icon size={14} />
                <span className="lyt-row__name">{p.title}</span>
                <span className={clsx("lyt-chip", `lyt-chip--${place.kind}`)}>{status}</span>
              </span>
              {detachable && monitors.length > 0 && !locked && (
                <select
                  className="lyt-select"
                  aria-label={t("editor.place.label", { title: p.title })}
                  value={target}
                  onChange={(e) => void layout.assignPanel(p.id, e.target.value)}
                >
                  <option value="dock">{t("editor.place.mainWindow")}</option>
                  {monitors.map((m) => (
                    <option key={m.key} value={m.key}>
                      {t("monitors.monitor", { n: m.index })}
                    </option>
                  ))}
                </select>
              )}
              {!locked && shown && place.kind !== "window" && (
                <button
                  type="button"
                  className="lyt-icon-btn"
                  title={place.kind === "floating" ? t("actions.dock") : t("actions.float")}
                  aria-label={place.kind === "floating" ? t("actions.dock") : t("actions.float")}
                  onClick={() => (place.kind === "floating" ? layout.dockPanel(p.id) : layout.floatPanel(p.id))}
                >
                  {place.kind === "floating" ? <SquareArrowDownLeft size={13} /> : <PictureInPicture2 size={13} />}
                </button>
              )}
              {!locked && detachable && (
                <button
                  type="button"
                  className="lyt-icon-btn"
                  title={place.kind === "window" ? t("actions.redock") : t("actions.detach")}
                  aria-label={place.kind === "window" ? t("actions.redock") : t("actions.detach")}
                  onClick={() => void (place.kind === "window" ? layout.redockPanel(p.id) : layout.detachPanel(p.id))}
                >
                  {place.kind === "window" ? <SquareArrowDownLeft size={13} /> : <AppWindow size={13} />}
                </button>
              )}
              <button
                type="button"
                className="lyt-icon-btn"
                disabled={locked}
                title={shown ? t("editor.hide") : t("editor.show")}
                aria-label={shown ? t("editor.hide") : t("editor.show")}
                onClick={() => (shown ? void layout.hidePanel(p.id) : layout.openPanel(p.id))}
              >
                {shown ? <Eye size={13} /> : <EyeOff size={13} />}
              </button>
            </li>
          );
        })}
      </ul>
    </section>
  );
}

const MAP = { width: 312, height: 150 };

function MonitorsSection() {
  const { t } = useT("layout");
  const monitors = useLayoutStore((s) => s.monitors);
  const popouts = useLayoutStore((s) => s.popouts);
  const mainMonitor = useLayoutStore((s) => s.mainMonitor);
  const mainPinned = useLayoutStore((s) => s.mainPinned);
  const geometry = useMemo(() => monitorMapGeometry(monitors, MAP), [monitors]);
  if (!canDetach()) {
    return (
      <section className="lyt-section">
        <h3 className="lyt-section__title">{t("editor.sections.monitors")}</h3>
        <p className="lyt-note">{t("monitors.browserOnly")}</p>
      </section>
    );
  }
  const mainKey = mainMonitor ?? monitors.find((m) => m.primary)?.key ?? null;
  return (
    <section className="lyt-section">
      <h3 className="lyt-section__title">{t("editor.sections.monitors")}</h3>
      <svg className="lyt-map" viewBox={`0 0 ${MAP.width} ${MAP.height}`} role="img" aria-label={t("monitors.mapLabel", { count: monitors.length })}>
        <defs>
          <linearGradient id="lyt-map-screen" x1="0" y1="0" x2="1" y2="1">
            <stop offset="0" stopColor="var(--ok-color-accent)" stopOpacity="0.22" />
            <stop offset="1" stopColor="var(--ok-color-violet)" stopOpacity="0.16" />
          </linearGradient>
        </defs>
        {geometry.map((g) => {
          const m = monitors.find((x) => x.key === g.key);
          if (!m) return null;
          const windows = popouts.filter((w) => w.monitor === m.key);
          const hasMain = m.key === mainKey;
          return (
            <g key={g.key} className={clsx("lyt-map__mon", hasMain && "lyt-map__mon--main")}>
              <rect x={g.x + 2} y={g.y + 2} width={Math.max(0, g.width - 4)} height={Math.max(0, g.height - 4)} rx="5" />
              <text x={g.x + 9} y={g.y + 17} className="lyt-map__num">
                {m.index}
              </text>
              {m.primary && (
                <g transform={`translate(${g.x + g.width - 18}, ${g.y + 7})`}>
                  <Star size={10} />
                </g>
              )}
              {[...(hasMain ? [t("monitors.mainWindowShort")] : []), ...windows.map((w) => PANELS.find((p) => p.id === w.panelId)?.title ?? w.panelId ?? "")]
                .slice(0, 4)
                .map((label, i) => (
                  <g key={`${label}-${i}`}>
                    <rect className="lyt-map__chip" x={g.x + 8} y={g.y + 24 + i * 15} width={Math.max(0, g.width - 16)} height={12} rx="3" />
                    <text className="lyt-map__chip-text" x={g.x + 12} y={g.y + 33 + i * 15}>
                      {label.length > 22 ? `${label.slice(0, 21)}…` : label}
                    </text>
                  </g>
                ))}
            </g>
          );
        })}
      </svg>
      {monitors.length > 0 && (
        <label className="lyt-field">
          <span>{t("monitors.mainOn")}</span>
          <select
            className="lyt-select"
            value={mainPinned && mainMonitor ? mainMonitor : ""}
            onChange={(e) => void layout.moveMainToMonitor(e.target.value || null)}
          >
            <option value="">{t("monitors.notPinned")}</option>
            {monitors.map((m) => (
              <option key={m.key} value={m.key}>
                {m.primary ? t("monitors.monitorPrimary", { n: m.index }) : t("monitors.monitor", { n: m.index })}
              </option>
            ))}
          </select>
        </label>
      )}
      {monitors.length > 1 ? (
        <div className="lyt-spread">
          <Button size="sm" variant="secondary" leftIcon={<MonitorSmartphone size={13} />} onClick={() => void layout.spreadAcrossMonitors()}>
            {t("monitors.spread")}
          </Button>
          <p className="lyt-note">{t("monitors.spreadHint")}</p>
        </div>
      ) : (
        <p className="lyt-note">{t("monitors.single")}</p>
      )}
    </section>
  );
}

/** The editor side sheet (mounted in App, shown while `editorOpen`). */
export function LayoutEditorPanel() {
  const { t } = useT(["layout", "common"]);
  const open = useLayoutStore((s) => s.editorOpen);
  const locked = useLayoutStore((s) => s.locked);
  const activeId = useLayoutStore((s) => s.activeId);
  const state = useLayoutStore((s) => s.state);
  const persistError = useLayoutStore((s) => s.persistError);
  const calm = useCalmMotion();
  const activeName = layoutName(activeId, state?.layouts.find((l) => l.id === activeId)?.name);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !useLayoutStore.getState().dialog) layout.openEditor(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open]);

  return (
    <AnimatePresence>
      {open && (
        <motion.aside
          className="lyt-sheet"
          role="dialog"
          aria-label={t("editor.title")}
          initial={calm ? false : { opacity: 0, x: 28, scale: 0.985 }}
          animate={{ opacity: 1, x: 0, scale: 1 }}
          exit={calm ? { opacity: 0, transition: { duration: 0 } } : { opacity: 0, x: 28, scale: 0.985 }}
          transition={{ type: "spring", stiffness: 420, damping: 36, mass: 0.8 }}
        >
          <header className="lyt-sheet__header">
            <div className="lyt-sheet__icon">
              <LayoutTemplate size={17} />
            </div>
            <div className="lyt-sheet__heading">
              <h2 className="lyt-sheet__title">{t("editor.title")}</h2>
              <p className="lyt-sheet__subtitle">{t("editor.subtitle", { name: activeName })}</p>
            </div>
            <button type="button" className="dialog__close" aria-label={t("common:actions.close")} onClick={() => layout.openEditor(false)}>
              <X size={16} />
            </button>
          </header>
          <div className="lyt-lock">
            <button
              type="button"
              role="switch"
              aria-checked={locked}
              className={clsx("lyt-switch", locked && "lyt-switch--on")}
              onClick={() => layout.setLocked(!locked)}
            >
              <span className="lyt-switch__knob">{locked ? <Lock size={11} /> : <Unlock size={11} />}</span>
            </button>
            <div>
              <div className="lyt-lock__title">{t("editor.lock")}</div>
              <div className="lyt-note">{t("editor.lockHint")}</div>
            </div>
          </div>
          {persistError && <p className="lyt-error">{t("toast.persistError", { error: persistError })}</p>}
          <div className="lyt-sheet__body">
            <PresetsSection />
            <PanelsSection />
            <MonitorsSection />
          </div>
          <footer className="lyt-sheet__footer">
            <Button variant="primary" onClick={() => layout.openEditor(false)}>
              {t("editor.done")}
            </Button>
          </footer>
        </motion.aside>
      )}
    </AnimatePresence>
  );
}
