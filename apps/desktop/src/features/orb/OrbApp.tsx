/**
 * Root of the orb window (ADR-0045). Its own small app on the same bundle:
 * connects to the core over RPC, follows settings (language, theme,
 * Performance Mode) and the UI configuration (orb preferences), keeps the
 * approvals live, renders the sphere and — on click — the Agent Hub flyout
 * next to it; right-click opens the orb menu. The shell (`orb.rs`) only
 * moves, resizes and click-through-gates the window.
 */
import type { AppSettings, Event, OrbAnimation, PerformanceMode } from "@orkestr/protocol";
import { TOPIC_GLOBAL } from "@orkestr/protocol";
import { clsx } from "clsx";
import { type PointerEvent as ReactPointerEvent, useCallback, useEffect, useMemo, useRef, useState } from "react";

import { bootstrapCore, maybeClient } from "../../core/client";
import { applyLanguagePreference, languagePreference, useT } from "../../i18n";
import { useAppStore } from "../../store/app";
import { useAiUiStore, useUiPrefs } from "../aiui/aiuiStore";
import { useAiUiRuntime } from "../aiui/useAiUiRuntime";
import { subscribeEffectiveMode } from "../performance/perfBroadcast";
import { initSecurityLive, useSecurityStore } from "../security/store";
import { useSettingsStore } from "../settings/settingsStore";
import { activeTheme, useThemeStore } from "../theme/themeStore";
import { useThemeEngine } from "../theme/ThemeRuntime";
import { channelSignal } from "../voice/jarvis/frame";
import { COLLAPSE_MS, reactorFps } from "../voice/jarvis/model";
import { useJarvisDebug, useJarvisState } from "../voice/jarvis/useJarvisState";
import { useOrbMainState, voicePopupWanted } from "./channel";
import { hubActions, useHubData } from "./hubData";
import { type HubItem, groupByProject, hubCounts } from "./hubModel";
import {
  type Box,
  type OrbSignals,
  HUB_SIZE,
  ORB_PAD,
  badgeText,
  isDragGesture,
  orbBadge,
  orbFps,
  orbHitShape,
  orbMood,
  orbMotion,
  orbPrefsOf,
} from "./model";
import { OrbHub } from "./OrbHub";
import { type OrbMenuAction, OrbMenu } from "./OrbMenu";
import { OrbSphere } from "./OrbSphere";
import { OrbVoicePopup } from "./OrbVoicePopup";
import { ORB_EVENTS, type OrbLayout, type OrbMoved, orbShell, savePosition } from "./shell";
import "./orb.css";

/** Size of the orb menu area (logical px). */
const MENU_SIZE = { width: 232, height: 300 } as const;
/** Size of the Jarvis voice card area for a reactor of `size` px (logical px). */
export function voiceCardSize(size: number): { width: number; height: number } {
  return { width: size + 12 + 300, height: Math.max(size + 10, 176) };
}

type Panel = "none" | "hub" | "menu" | "voice";

/** Settings (language, theme, chosen performance mode) for the orb window. */
function useOrbSettings(ready: boolean): PerformanceMode {
  const [chosen, setChosen] = useState<PerformanceMode>("Balanced");
  const [effective, setEffective] = useState<PerformanceMode | null>(null);
  useEffect(() => subscribeEffectiveMode(setEffective), []);
  useEffect(() => {
    if (!ready) return undefined;
    const apply = (s: AppSettings | null) => {
      if (!s) return;
      if (s.language !== languagePreference()) void applyLanguagePreference(s.language);
      if (s.themeId && s.themeId !== useAppStore.getState().themeId) useAppStore.getState().setThemeId(s.themeId);
      setChosen(s.performanceMode);
    };
    const offStore = useSettingsStore.subscribe((s, prev) => {
      if (s.settings !== prev.settings) apply(s.settings);
    });
    void useSettingsStore.getState().load();
    const c = maybeClient();
    const offEvents = c?.subscribe([TOPIC_GLOBAL], (ev: Event) => {
      if (ev.kind === "SettingsChanged") useSettingsStore.getState().applyRemote(ev.payload.settings);
    });
    return () => {
      offStore();
      offEvents?.();
    };
  }, [ready]);
  return effective ?? chosen;
}

function useReducedMotion(): boolean {
  const [reduced, setReduced] = useState(() => {
    try {
      return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    } catch {
      return false;
    }
  });
  useEffect(() => {
    let mq: MediaQueryList;
    try {
      mq = window.matchMedia("(prefers-reduced-motion: reduce)");
    } catch {
      return undefined;
    }
    const on = () => setReduced(mq.matches);
    mq.addEventListener?.("change", on);
    return () => mq.removeEventListener?.("change", on);
  }, []);
  return reduced;
}

function useDocumentHidden(): boolean {
  const [hidden, setHidden] = useState(() => (typeof document !== "undefined" ? document.hidden : false));
  useEffect(() => {
    const on = () => setHidden(document.hidden);
    document.addEventListener("visibilitychange", on);
    return () => document.removeEventListener("visibilitychange", on);
  }, []);
  return hidden;
}

/**
 * `window.__orkestrDebug.orb.setSignals({...})` when the debug-hooks flag is on
 * (`localStorage["orkestr.debugHooks"] = "1"`, like the voice hook): forces
 * moods for screenshots and manual checks. Off by default.
 */
function useOrbDebugHook(): Partial<OrbSignals> | null {
  const [forced, setForced] = useState<Partial<OrbSignals> | null>(null);
  useEffect(() => {
    let on = false;
    try {
      on = window.localStorage.getItem("orkestr.debugHooks") === "1";
    } catch {
      on = false;
    }
    if (!on) return undefined;
    const w = window as unknown as { __orkestrDebug?: Record<string, unknown> };
    w.__orkestrDebug = { ...(w.__orkestrDebug ?? {}), orb: { setSignals: (p: Partial<OrbSignals> | null) => setForced(p) } };
    return () => {
      if (w.__orkestrDebug) delete w.__orkestrDebug.orb;
    };
  }, []);
  return forced;
}

/** Browser preview (`?orb=1`): no shell, the hub opens to the right of the sphere. */
function previewLayout(panel: Panel, box: number): OrbLayout {
  return panel === "none" ? { orbX: 0, orbY: 0, hubX: 0, hubY: 0, side: "right" } : { orbX: 0, orbY: 0, hubX: box + 8, hubY: 0, side: "right" };
}

export function OrbApp() {
  const { t } = useT("orb");
  const [ready, setReady] = useState(false);
  const connected = useAppStore((s) => s.connection.state === "open");
  const prefsDoc = useUiPrefs();
  const prefs = orbPrefsOf(prefsDoc.orb);
  const theme = useThemeStore(activeTheme);
  const themeKey = useMemo(() => `${theme.id}:${theme.mode}:${JSON.stringify(theme.tokens)}`, [theme]);
  useThemeEngine(ready);
  const mode = useOrbSettings(ready);
  useAiUiRuntime(connected, false);
  const reducedMotion = useReducedMotion();
  const hidden = useDocumentHidden();
  const signal = useMemo(() => channelSignal(), []);
  const main = useOrbMainState(signal.push);
  const jarvisForced = useJarvisDebug();

  const [panel, setPanel] = useState<Panel>("none");
  const [layout, setLayout] = useState<OrbLayout>({ orbX: 0, orbY: 0, hubX: 0, hubY: 0, side: "right" });
  const [hovered, setHovered] = useState(false);
  const [dragging, setDragging] = useState(false);
  const debugSignals = useOrbDebugHook();
  const [acknowledged, setAcknowledged] = useState<ReadonlySet<string>>(() => new Set());
  const press = useRef<{ x: number; y: number; dragged: boolean } | null>(null);
  const panelRef = useRef<HTMLDivElement | null>(null);
  const sphereRef = useRef<HTMLDivElement | null>(null);

  // Boot.
  useEffect(() => {
    document.documentElement.dataset["okWindow"] = "orb";
    bootstrapCore().then(
      () => setReady(true),
      (e: unknown) => console.error("orb: core bootstrap failed", e),
    );
  }, []);
  useEffect(() => (connected ? initSecurityLive() : undefined), [connected]);

  // Data.
  const hub = useHubData(panel === "hub");
  const projects = useAppStore((s) => s.projects);
  const groups = useMemo(() => groupByProject(hub.items, projects), [hub.items, projects]);
  const pending = useSecurityStore((s) => s.pending);
  const allStopped = useSecurityStore((s) => s.flags.allStopped);
  const counts = hubCounts(hub.items, acknowledged);
  const signals: OrbSignals = {
    connected,
    working: counts.working,
    approvals: pending.filter((a) => a.status === "Pending").length,
    errors: counts.errors,
    voicePhase: main.state.voice.phase,
    speaking: main.state.voice.speaking,
    allStopped,
    ...debugSignals,
  };
  const mood = orbMood(signals);
  const badge = orbBadge(signals, prefs.showBadge);
  const motion = orbMotion({
    pref: prefs.animation,
    mode,
    reducedMotion,
    hidden,
    appActive: main.state.appActive,
    hovered: hovered || dragging,
    hubOpen: panel !== "none",
    voice: mood === "listening" || mood === "speaking",
  });
  const fps = orbFps(motion, mode);
  const size = prefs.size;
  const box = size + ORB_PAD * 2;

  // Seen errors stop tinting the sphere once the hub showed them.
  useEffect(() => {
    if (panel !== "hub") return;
    const errs = hub.items.filter((i) => i.status === "error" && !acknowledged.has(i.id));
    if (errs.length) setAcknowledged((prev) => new Set([...prev, ...errs.map((i) => i.id)]));
  }, [panel, hub.items, acknowledged]);

  // Background voice command: the Jarvis card opens next to the orb and
  // collapses back into it when the command is done.
  const voiceState = main.state.voice;
  const appActive = main.state.appActive;
  const jarvisState = useJarvisState(voiceState.phase, voiceState.speaking, voiceState.source);
  const [leaving, setLeaving] = useState(false);
  useEffect(() => {
    const want = jarvisForced !== null || voicePopupWanted(voiceState, appActive, panel === "voice");
    if (want && (panel === "none" || (panel === "voice" && leaving))) {
      setLeaving(false);
      setPanel("voice");
    } else if (!want && panel === "voice" && !leaving) setLeaving(true);
  }, [voiceState, appActive, panel, leaving, jarvisForced]);
  useEffect(() => {
    if (!leaving) return undefined;
    const timer = window.setTimeout(() => {
      setLeaving(false);
      setPanel((cur) => (cur === "voice" ? "none" : cur));
    }, COLLAPSE_MS);
    return () => window.clearTimeout(timer);
  }, [leaving]);
  // Dragging the card moves the orb with it; afterwards the card re-picks its side.
  const [relayout, setRelayout] = useState(0);
  const voiceFps = reactorFps(mode, reducedMotion, hidden);
  const voiceSize = prefs.voiceSize;
  const voiceDims = voiceCardSize(voiceSize);

  // Window area for the open panel (the shell grows the window next to the sphere).
  const openPanel = useCallback((next: Panel) => setPanel(next), []);
  const close = useCallback(() => setPanel("none"), []);
  useEffect(() => {
    const dims: readonly [number, number] | null =
      panel === "hub"
        ? [HUB_SIZE.width, HUB_SIZE.height]
        : panel === "menu"
          ? [MENU_SIZE.width, MENU_SIZE.height]
          : panel === "voice"
            ? [voiceDims.width, voiceDims.height]
            : null;
    if (!orbShell.available()) {
      setLayout(previewLayout(panel, box));
      return undefined;
    }
    let cancelled = false;
    orbShell.setLayout(dims).then(
      (l) => {
        if (l && !cancelled) setLayout(l);
      },
      (e: unknown) => console.warn("orb_set_layout", e),
    );
    return () => {
      cancelled = true;
    };
  }, [panel, box, voiceDims.width, voiceDims.height, relayout]);

  // Shell signals.
  useEffect(() => {
    const subs = [
      orbShell.on<boolean>(ORB_EVENTS.hover, setHovered),
      orbShell.on<boolean>(ORB_EVENTS.drag, (d) => {
        setDragging(d);
        if (!d) setRelayout((n) => n + 1);
      }),
      orbShell.on<OrbMoved>(ORB_EVENTS.moved, (p) => savePosition({ monitor: p.monitor, x: p.x, y: p.y })),
      orbShell.on<null>(ORB_EVENTS.hub, () => setPanel((cur) => (cur === "hub" ? "none" : "hub"))),
    ];
    return () => subs.forEach((s) => void s.then((off) => off()));
  }, []);

  // Clicking elsewhere (the window loses focus) closes the hub / menu.
  useEffect(() => {
    if (panel === "none" || panel === "voice") return undefined;
    const onBlur = () => close();
    window.addEventListener("blur", onBlur);
    return () => window.removeEventListener("blur", onBlur);
  }, [panel, close]);

  // Report the interactive shape (sphere circle + open panel box).
  const [panelBox, setPanelBox] = useState<Box | null>(null);
  useEffect(() => {
    if (panel === "none") {
      setPanelBox(null);
      return undefined;
    }
    const measure = () => {
      const r = panelRef.current?.getBoundingClientRect();
      setPanelBox(r && r.width > 0 ? { x: r.left, y: r.top, w: r.width, h: r.height } : null);
    };
    const raf = requestAnimationFrame(measure);
    const timer = window.setTimeout(measure, 300);
    return () => {
      cancelAnimationFrame(raf);
      window.clearTimeout(timer);
    };
  }, [panel, layout]);
  useEffect(() => {
    const shape = orbHitShape({ x: layout.orbX, y: layout.orbY }, size, [panelBox]);
    // Orb switched off (popup only): the hidden sphere takes no clicks.
    if (!prefs.enabled) shape.circles = [];
    orbShell.setShape(shape).catch((e: unknown) => console.warn("orb_set_shape", e));
  }, [layout.orbX, layout.orbY, size, panelBox, prefs.enabled]);

  // Escape anywhere closes; the hub handles its own keys first.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !e.defaultPrevented && panel !== "none") close();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [panel, close]);

  const patchOrb = (path: string, value: string | number | boolean, summary: string) =>
    void useAiUiStore.getState().applyUserPatch([{ op: "replace", path: `/prefs/orb/${path}`, value }], summary);

  const onMenu = (a: OrbMenuAction) => {
    switch (a.kind) {
      case "hub":
        openPanel("hub");
        return;
      case "main":
        close();
        void orbShell.toMain("open-main");
        return;
      case "animation":
        patchOrb("animation", a.value satisfies OrbAnimation, t("summaries.animation", { value: t(`animation.${a.value}`) }));
        return;
      case "badge":
        patchOrb("showBadge", !prefs.showBadge, t("summaries.badge"));
        return;
      case "settings":
        close();
        void orbShell.toMain("open-settings", { section: "orb" });
        return;
      case "hide":
        close();
        patchOrb("enabled", false, t("summaries.hidden"));
        return;
    }
  };

  const onAction = (item: HubItem, action: "pause" | "resume" | "stop" | "restart" | "transcript") => hubActions[action](item);

  const onPointerDown = (e: ReactPointerEvent) => {
    if (e.button !== 0) return;
    press.current = { x: e.clientX, y: e.clientY, dragged: false };
  };
  const onPointerMove = (e: ReactPointerEvent) => {
    const p = press.current;
    if (!p || p.dragged || panel !== "none" || !isDragGesture(e.clientX - p.x, e.clientY - p.y)) return;
    p.dragged = true;
    void orbShell.startDragging().catch((err: unknown) => console.warn("startDragging", err));
  };
  const onPointerUp = () => {
    const p = press.current;
    press.current = null;
    if (p && !p.dragged) openPanel(panel === "hub" ? "none" : "hub");
  };

  const label = t("orb.label", { state: t(`mood.${mood}`) });
  const panelStyle = { left: layout.hubX, top: layout.hubY };

  return (
    <div className={clsx("orb-root", `orb-root--${mood}`, `orb-root--${motion}`)}>
      <div
        ref={sphereRef}
        className={clsx("orb", hovered && "orb--hover", dragging && "orb--drag", panel === "hub" && "orb--open", !prefs.enabled && "orb--ghost")}
        style={{ left: layout.orbX, top: layout.orbY, width: box, height: box }}
        role="button"
        tabIndex={0}
        aria-label={label}
        aria-haspopup="dialog"
        aria-expanded={panel === "hub"}
        title={label}
        data-orb-mood={mood}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            openPanel(panel === "hub" ? "none" : "hub");
          }
          if (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) {
            e.preventDefault();
            openPanel("menu");
          }
        }}
        onContextMenu={(e) => {
          e.preventDefault();
          openPanel(panel === "menu" ? "none" : "menu");
        }}
      >
        <OrbSphere
          size={size}
          pad={ORB_PAD}
          mood={mood}
          motion={motion}
          fps={fps}
          rings={Math.min(3, Math.max(1, counts.working))}
          level={main.state.voice.level}
          themeKey={themeKey}
          accent={theme.robot?.accent ?? null}
          lit={hovered || panel !== "none"}
        />
        {badge && (
          <span className={clsx("orb-badge", `orb-badge--${badge.tone}`)} aria-hidden="true">
            {badgeText(badge.count)}
          </span>
        )}
      </div>
      {panel === "hub" && (
        <div ref={panelRef} className={clsx("orb-panel", `orb-panel--${layout.side}`)} style={{ ...panelStyle, width: HUB_SIZE.width, height: HUB_SIZE.height }}>
          <OrbHub
            groups={groups}
            items={hub.items}
            loaded={hub.loaded}
            voice={main.state.voice}
            onPtt={main.ptt}
            onAction={onAction}
            onMessage={hubActions.message}
            onMessageManager={hubActions.messageManager}
            onOpenMain={() => {
              close();
              void orbShell.toMain("open-main");
            }}
            onOpenSettings={() => {
              close();
              void orbShell.toMain("open-settings", { section: "orb" });
            }}
            onOpenApprovals={() => void orbShell.toMain("open-approvals")}
            onClose={() => {
              close();
              sphereRef.current?.focus({ preventScroll: true });
            }}
          />
        </div>
      )}
      {panel === "voice" && (
        <div ref={panelRef} className={clsx("orb-panel orb-panel--voice", `orb-panel--${layout.side}`)} style={{ ...panelStyle, width: voiceDims.width, height: voiceDims.height }}>
          <OrbVoicePopup
            voice={voiceState}
            state={jarvisState}
            forced={jarvisForced}
            onAction={main.voiceAction}
            onOpenMain={() => {
              main.voiceAction("cancel");
              void orbShell.toMain("open-main");
            }}
            size={voiceSize}
            fps={voiceFps}
            motion={!reducedMotion && voiceFps > 0}
            signal={signal}
            themeKey={themeKey}
            accent={theme.robot?.accent ?? null}
            side={layout.side}
            leaving={leaving}
            onReactorPointerDown={(e) => {
              if (e.button !== 0) return;
              void orbShell.startDragging().catch((err: unknown) => console.warn("startDragging", err));
            }}
          />
        </div>
      )}
      {panel === "menu" && (
        <div ref={panelRef} className={clsx("orb-panel orb-panel--menu", `orb-panel--${layout.side}`)} style={{ ...panelStyle, width: MENU_SIZE.width }}>
          <OrbMenu animation={prefs.animation} showBadge={prefs.showBadge} onAction={onMenu} onClose={close} />
        </div>
      )}
    </div>
  );
}
