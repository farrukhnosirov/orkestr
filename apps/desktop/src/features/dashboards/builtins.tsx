/**
 * Native rendering of built-in widgets (`builtin:<id>`): the Mission Control
 * widgets (same components, same shared snapshot context) plus the
 * dashboard-only `note` widget. Plugin widgets never render here — they go
 * through the sandboxed {@link WidgetFrame}.
 */
import { Badge } from "@orkestr/ui";
import { StickyNote } from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { type ReactNode, useMemo, useState } from "react";

import { useT } from "../../i18n";
import { useAppStore } from "../../store/app";
// The Mission Control store must initialise before its widget registry (import cycle).
import "../../panels/MissionControl/store";
import type { WidgetSize } from "../../panels/MissionControl/arrangement";
import { MissionContext, type MissionContextValue } from "../../panels/MissionControl/context";
import { useDashboardSnapshot } from "../../panels/MissionControl/data";
import { useVisualBudget } from "../../panels/MissionControl/visual";
import { getWidget } from "../../panels/MissionControl/widgets/registry";
import "../../panels/MissionControl/missionControl.css";

export const BUILTIN_PREFIX = "builtin:";

export function isBuiltin(widgetId: string): boolean {
  return widgetId.startsWith(BUILTIN_PREFIX);
}

/** Mission Control widgets need the shared snapshot context ({@link BuiltinProvider}). */
export function needsMissionContext(widgetId: string): boolean {
  return isBuiltin(widgetId) && widgetId !== `${BUILTIN_PREFIX}note`;
}

/** Mission Control size closest to a grid width (for widgets that adapt to `size`). */
export function sizeForWidth(w: number): WidgetSize {
  if (w >= 12) return "full";
  if (w >= 8) return "xl";
  if (w >= 6) return "l";
  if (w >= 4) return "m";
  return "s";
}

/** Icon of a built-in widget (Mission Control manifest or the note icon). */
export function builtinIcon(widgetId: string): LucideIcon | null {
  const id = widgetId.slice(BUILTIN_PREFIX.length);
  if (id === "note") return StickyNote;
  return getWidget(id)?.icon ?? null;
}

/** Localized title of a built-in widget (falls back to the core title). */
export function builtinTitle(widgetId: string, fallback: string): string {
  const id = widgetId.slice(BUILTIN_PREFIX.length);
  if (id === "note") return fallback;
  return getWidget(id)?.title ?? fallback;
}

/** Shared snapshot context for Mission Control widgets on a dashboard. */
export function BuiltinProvider({ children }: { children: ReactNode }) {
  const snapshot = useDashboardSnapshot();
  const budget = useVisualBudget();
  const activeProjectId = useAppStore((s) => s.activeProjectId);
  const [picked, setPicked] = useState<string | null>(null);
  const focusProjectId = picked ?? activeProjectId ?? snapshot.data?.projects?.[0]?.projectId ?? null;
  const ctx = useMemo<MissionContextValue>(
    () => ({ snapshot, budget, focusProjectId, setFocusProjectId: setPicked }),
    [snapshot, budget, focusProjectId],
  );
  return <MissionContext.Provider value={ctx}>{children}</MissionContext.Provider>;
}

const NOTE_TONES = new Set(["neutral", "info", "success", "warning", "danger"]);

function NoteWidget({ config }: { config: unknown }) {
  const { t } = useT("dashboards");
  const c = (typeof config === "object" && config !== null ? config : {}) as { text?: unknown; tone?: unknown };
  const text = typeof c.text === "string" ? c.text : "";
  const tone = typeof c.tone === "string" && NOTE_TONES.has(c.tone) ? c.tone : "neutral";
  return (
    <div className={`dsh-note dsh-note--${tone}`}>
      {text ? <p className="dsh-note__text">{text}</p> : <Badge tone="neutral">{t("note.empty")}</Badge>}
    </div>
  );
}

/** Renders a built-in widget body. */
export function BuiltinWidget({ widgetId, w, config }: { widgetId: string; w: number; config: unknown }) {
  const { t } = useT("dashboards");
  const id = widgetId.slice(BUILTIN_PREFIX.length);
  if (id === "note") return <NoteWidget config={config} />;
  const def = getWidget(id);
  if (!def) return <div className="mc-error" role="alert">{t("widget.unknown", { id: widgetId })}</div>;
  const Component = def.component;
  return <Component size={sizeForWidth(w)} />;
}
