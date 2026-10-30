/**
 * Startup animation overlay (ADR-0053). Sits over the connection screen while
 * the core starts; ends as soon as both the animation's minimum and the core
 * readiness are met (see `sequence.ts`). Click / Esc / any key skips.
 *
 * The canvas (`renderer.ts`) draws the background and the reactor; the HUD
 * texts are DOM (crisp, translatable, read by screen readers) and are typed
 * out from the same animation clock by a per-frame update of `textContent`.
 */
import { clsx } from "clsx";
import { useEffect, useMemo, useRef } from "react";
import { createPortal } from "react-dom";

import { formatPercent, useT } from "../../i18n";
import { useAppStore } from "../../store/app";
import { updatesApi, updatesAvailableHere } from "../updates/api";
import { say } from "../voice/controller";
import { useVoiceStore } from "../voice/store";
import { bootTexts, type BootPlan } from "./plan";
import { readBootPrefs } from "./prefs";
import { BootRenderer, igniteAt, readPalette, type FlyTarget } from "./renderer";
import { BootSequencer, type BootFrameState, type BootVariant, typed } from "./sequence";
import { bootSoundAllowed, playBootSound } from "./sound";
import { resetBootSteps, runBootSteps, STEP_IDS, stepsProgress, stepsSettled, useBootStore, type StepId } from "./store";
import "./boot.css";

type Variant = Exclude<BootVariant, "none">;

export interface BootOverlayProps {
  plan: BootPlan & { variant: Variant };
  /** Core connection ready. */
  ready: boolean;
  /** Assistant mode: compact layout. */
  compact: boolean;
  /** Settings preview: no version bookkeeping, no greeting. */
  preview?: boolean;
  onDone: () => void;
}

/** Script (animation seconds) of the HUD lines per version. */
function script(v: Variant, gate: number) {
  if (v === "full") return { title: 0.2, cps: 34, steps: [0.55, 0.95, 1.35, 1.7, 2.05], all: gate + 0.02, welcome: gate + 0.15, welcomeCps: 30, sub: gate + 0.55, skip: 0.9 };
  if (v === "short") return { title: 0, cps: 80, steps: null, all: gate, welcome: gate, welcomeCps: 70, sub: gate + 0.2, skip: 99 };
  return { title: 0, cps: 1e4, steps: null, all: gate, welcome: gate, welcomeCps: 1e4, sub: gate, skip: 99 };
}

const FLY_SELECTORS = ".titlebar__brand .ok-logo, .as-titlebar__brand .ok-logo";

function flyTarget(): FlyTarget | null {
  const el = document.querySelector(FLY_SELECTORS);
  if (!el) return null;
  const r = el.getBoundingClientRect();
  if (r.width <= 0) return null;
  return { x: r.left + r.width / 2, y: r.top + r.height / 2, size: Math.max(r.width, r.height) * 1.25 };
}

/** Debug hook (`localStorage["orkestr.debugHooks"] = "1"`): freeze a frame for screenshots. */
interface BootDebug {
  freeze: (s: Partial<BootFrameState> | null, now?: number) => void;
  info: () => unknown;
}

const PLAYED_KEY = "orkestr.boot.played";

/** A sequence already played in this app session (a page reload keeps the core's startup report). */
function playedThisSession(): boolean {
  try {
    return window.sessionStorage.getItem(PLAYED_KEY) === "1";
  } catch {
    return false;
  }
}

function markPlayed(): void {
  try {
    window.sessionStorage.setItem(PLAYED_KEY, "1");
  } catch {
    /* storage unavailable */
  }
}

function debugOn(): boolean {
  try {
    return window.localStorage.getItem("orkestr.debugHooks") === "1";
  } catch {
    return false;
  }
}

export function BootOverlay({ plan, ready, compact, preview = false, onDone }: BootOverlayProps) {
  const { t } = useT("boot");
  const variant = plan.variant;
  const rootRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const seqRef = useRef<BootSequencer | null>(null);
  if (!seqRef.current) seqRef.current = new BootSequencer(variant, performance.now());
  const doneRef = useRef(onDone);
  doneRef.current = onDone;
  const readyRef = useRef(ready);
  readyRef.current = ready;
  const texts = useMemo(() => bootTexts(plan.env), [plan.env]);

  // Readiness → sequencer; failed start → fade out (the connection screen shows the error).
  useEffect(() => {
    if (ready) seqRef.current?.markReady(performance.now());
  }, [ready]);
  useEffect(
    () =>
      useAppStore.subscribe((s) => {
        if (s.connection.state === "closed" && !readyRef.current) seqRef.current?.fail(performance.now());
      }),
    [],
  );

  // The core's startup check (`app-version.json`) may know better: a regular
  // start that is really the first one of an installed / updated version.
  useEffect(() => {
    if (preview || variant !== "short" || playedThisSession() || !updatesAvailableHere()) return;
    updatesApi
      .status()
      .then((st) => {
        const kind = st.startup?.start.kind;
        if (kind && kind !== "same" && seqRef.current?.upgrade("full", performance.now())) {
          rootRef.current?.classList.replace("boot--short", "boot--full");
        }
      })
      .catch(() => undefined);
  }, [preview, variant]);

  // Real startup steps.
  useEffect(() => {
    if (preview) {
      resetBootSteps();
      return runBootSteps();
    }
    if (!ready) return undefined;
    return runBootSteps();
  }, [ready, preview]);
  useEffect(
    () =>
      useBootStore.subscribe((s) => {
        if (stepsSettled(s.steps)) seqRef.current?.markSettled(performance.now());
      }),
    [],
  );

  // Boot sound.
  useEffect(() => {
    const prefs = readBootPrefs();
    const first = preview || plan.firstLaunch;
    if (!bootSoundAllowed(prefs.sound === "off" ? "off" : preview ? "always" : prefs.sound, first, plan.env)) return;
    void playBootSound(plan.env.volumePct, igniteAt(variant) || 0.25, variant !== "full");
  }, [plan, preview, variant]);

  // Skip: click / Esc / any key.
  useEffect(() => {
    const skip = (e: Event) => {
      if (e instanceof KeyboardEvent && (e.key === "Escape" || e.key === " " || e.key === "Enter")) e.preventDefault();
      seqRef.current?.skip(performance.now());
    };
    window.addEventListener("keydown", skip, { capture: true });
    return () => window.removeEventListener("keydown", skip, { capture: true });
  }, []);

  // Animation loop.
  useEffect(() => {
    const root = rootRef.current;
    const canvas = canvasRef.current;
    const seq = seqRef.current;
    if (!root || !canvas || !seq) return undefined;
    let renderer: BootRenderer | null = null;
    try {
      renderer = new BootRenderer(canvas);
    } catch {
      renderer = null; // no canvas: the HUD texts still play
    }
    const q = <T extends Element>(sel: string) => root.querySelector<T>(sel);
    const title = q<HTMLElement>("[data-boot=title]");
    const stepEls = STEP_IDS.map((id) => ({ id, line: q<HTMLElement>(`[data-step=${id}]`), txt: q<HTMLElement>(`[data-step=${id}] .boot__txt`), tag: q<HTMLElement>(`[data-step=${id}] .boot__tag`) }));
    const all = q<HTMLElement>("[data-boot=all]");
    const welcome = q<HTMLElement>("[data-boot=welcome]");
    const caret = q<HTMLElement>(".boot__caret");
    const sub = q<HTMLElement>("[data-boot=sub]");
    const hud = q<HTMLElement>(".boot__hud");
    const greet = q<HTMLElement>(".boot__greet");
    const meterBar = q<HTMLElement>(".boot__bar i");
    const meterPct = q<HTMLElement>(".boot__pct");
    const skipEl = q<HTMLElement>(".boot__skip");

    const set = (el: HTMLElement | null, text: string) => {
      if (el && el.textContent !== text) el.textContent = text;
    };
    const show = (el: HTMLElement | null, on: boolean) => {
      if (el) el.dataset.on = on ? "1" : "0";
    };

    const layout = () => {
      const w = root.clientWidth;
      const h = root.clientHeight;
      const L = renderer?.resize(w, h, window.devicePixelRatio || 1, compact) ?? { cx: w / 2, cy: h * 0.42, R: 120, compact };
      root.style.setProperty("--boot-cx", `${L.cx}px`);
      root.style.setProperty("--boot-cy", `${L.cy}px`);
      root.style.setProperty("--boot-r", `${L.R}px`);
      root.classList.toggle("boot--compact", L.compact);
      renderer?.setPalette(readPalette(null));
    };
    layout();
    const ro = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(layout);
    ro?.observe(root);
    let palTimer = window.setInterval(() => renderer?.setPalette(readPalette(null)), 1500);

    const start = performance.now();
    let target: FlyTarget | null = null;
    let frozen: { s: Partial<BootFrameState>; now?: number } | null = null;
    let finished = false;
    let raf = 0;
    let lastStatic = 0;
    let greeted = false;
    const isStatic = variant === "static";

    if (debugOn()) {
      const w = window as unknown as { __orkestrDebug?: Record<string, unknown> };
      const hook: BootDebug = { freeze: (s, now) => (frozen = s ? { s, now } : null), info: () => ({ palette: renderer?.palette, layout: renderer?.layout }) };
      w.__orkestrDebug = { ...(w.__orkestrDebug ?? {}), boot: hook };
    }

    const finish = (spoke: boolean) => {
      if (finished) return;
      finished = true;
      if (!preview) markPlayed();
      useBootStore.getState().finish(spoke);
      doneRef.current();
    };

    const greetAloud = () => {
      if (greeted || preview) return;
      greeted = true;
      if (!readBootPrefs().voice) return;
      const speak = () => {
        if (!useVoiceStore.getState().config?.spokenReplies) return;
        void say(texts.spoken);
        useBootStore.setState({ spokeGreeting: true });
      };
      if (useVoiceStore.getState().config) return speak();
      const off = useVoiceStore.subscribe((s) => {
        if (!s.config) return;
        off();
        speak();
      });
      window.setTimeout(off, 5000);
    };

    const frame = () => {
      const nowMs = performance.now();
      raf = requestAnimationFrame(frame);
      let s = seq.state(nowMs);
      let now = (nowMs - start) / 1000;
      if (frozen) {
        s = { phase: "online", t: 0, hold: 0, exitP: 0, fly: true, ...frozen.s };
        if (frozen.now !== undefined) now = frozen.now;
      }
      if (s.phase === "done" && !frozen) {
        cancelAnimationFrame(raf);
        finish(useBootStore.getState().spokeGreeting);
        return;
      }
      if ((s.phase === "online" || s.phase === "exit") && !frozen) greetAloud();
      if (s.phase === "exit" && !target && s.fly) target = flyTarget();
      const steps = useBootStore.getState().steps;
      const tl = seq.timeline;
      const sc = script(seq.variant, tl.gate);

      // Canvas (the static version redraws ~10×/s).
      if (renderer && (!isStatic || nowMs - lastStatic > 100 || s.phase === "exit")) {
        lastStatic = nowMs;
        renderer.draw({ s, variant: seq.variant, now, target: frozen ? flyTarget() : target });
      }

      // HUD.
      const tt = s.t;
      const past = s.phase !== "intro" && s.phase !== "holding";
      set(title, typed(texts.title, tt, sc.title, sc.cps));
      show(title, tt > sc.title);
      for (let i = 0; i < stepEls.length; i++) {
        const el = stepEls[i]!;
        const at = sc.steps ? sc.steps[i]! : null;
        const visible = at !== null ? tt > at : !past && s.phase === "holding" && el.id === "core";
        show(el.line, visible);
        if (!visible) continue;
        const full = texts.steps[el.id as StepId];
        const txt = at !== null ? typed(full, tt, at, sc.cps) : full;
        set(el.txt, txt);
        const st = steps[el.id as StepId];
        const tag = txt.length === full.length ? texts.status[st] : "";
        set(el.tag, tag);
        if (el.tag) el.tag.dataset.state = st;
      }
      set(all, past ? typed(texts.allOnline, tt, sc.all, sc.cps) : "");
      show(all, past);
      const w = past ? typed(texts.welcome, tt, sc.welcome, sc.welcomeCps) : "";
      set(welcome, w);
      show(greet, past);
      if (caret) caret.dataset.on = past && w.length < texts.welcome.length ? "1" : "0";
      show(sub, past && tt > sc.sub);
      const prog = Math.min(1, stepsProgress(steps) * (s.phase === "intro" && seq.variant === "full" ? Math.min(1, tt / 2.4) : 1));
      if (meterBar) meterBar.style.transform = `scaleX(${prog.toFixed(3)})`;
      set(meterPct, formatPercent(prog));
      show(skipEl, tt > sc.skip && s.phase !== "exit");
      // Exit: HUD fades, the overlay stops catching clicks.
      const fade = s.phase === "exit" ? Math.max(0, 1 - s.exitP * 2.2) : 1;
      if (hud) hud.style.opacity = fade.toFixed(3);
      root.dataset.phase = s.phase;
    };
    raf = requestAnimationFrame(frame);
    return () => {
      cancelAnimationFrame(raf);
      ro?.disconnect();
      window.clearInterval(palTimer);
      palTimer = 0;
      const w = window as unknown as { __orkestrDebug?: Record<string, unknown> };
      if (w.__orkestrDebug) delete w.__orkestrDebug.boot;
    };
  }, [variant, compact, preview, texts]);

  // Portalled to <body>: above the app's own overlays (onboarding, dialogs), which live there too.
  const node = (
    <div
      ref={rootRef}
      className={clsx("boot", `boot--${variant}`, compact && "boot--compact")}
      role="status"
      aria-live="polite"
      aria-label={t("label")}
      data-testid="boot-overlay"
      onPointerDown={() => seqRef.current?.skip(performance.now())}
    >
      <canvas ref={canvasRef} className="boot__canvas" aria-hidden="true" />
      <div className="boot__hud">
        <div className="boot__log" aria-hidden="true">
          <div className="boot__line boot__line--title" data-boot="title" />
          {STEP_IDS.map((id) => (
            <div key={id} className="boot__line" data-step={id}>
              <span className="boot__txt" />
              <span className="boot__tag" />
            </div>
          ))}
          <div className="boot__line boot__line--all" data-boot="all" />
        </div>
        <div className="boot__greet">
          <div className="boot__welcome">
            <span data-boot="welcome" />
            <i className="boot__caret" aria-hidden="true" />
          </div>
          <div className="boot__subtitle" data-boot="sub">
            {texts.subtitle}
          </div>
        </div>
        <div className="boot__meter" aria-hidden="true">
          <div className="boot__bar">
            <i />
          </div>
          <span className="boot__pct" />
        </div>
        <div className="boot__skip">{texts.skip}</div>
      </div>
    </div>
  );
  return typeof document === "undefined" ? node : createPortal(node, document.body);
}
