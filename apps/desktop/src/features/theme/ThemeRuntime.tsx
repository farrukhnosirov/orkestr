/**
 * Theme runtime: applies the active theme (editor preview > device/global
 * config > cached) to `:root` under the current Performance Mode and the OS
 * reduced-motion preference, and renders the app background.
 */
import type { PerformanceMode } from "@orkestr/protocol";
import { type EffectiveVisuals, applyTheme, effectiveVisuals } from "@orkestr/ui";
import type { ThemeConfig } from "@orkestr/protocol";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { create } from "zustand";

import { useAppStore } from "../../store/app";
import { usePerfStore } from "../performance/perfStore";
import { useSettingsStore } from "../settings/settingsStore";
import { type SceneHandle, type SceneParams, runScene, scenePalette } from "./backgrounds/scene";
import { activeTheme, useThemeStore } from "./themeStore";
import "./theme.css";

/** Effective visuals of the applied theme (read by the backdrop and the editor). */
export const useVisualsStore = create<{ visuals: EffectiveVisuals | null; performanceMode: PerformanceMode }>()(() => ({
  visuals: null,
  performanceMode: "Balanced",
}));

/** `prefers-reduced-motion: reduce`, live. */
export function useReducedMotion(): boolean {
  const query = "(prefers-reduced-motion: reduce)";
  const [reduced, setReduced] = useState(() => typeof window.matchMedia === "function" && window.matchMedia(query).matches);
  useEffect(() => {
    if (typeof window.matchMedia !== "function") return;
    const mq = window.matchMedia(query);
    const on = () => setReduced(mq.matches);
    mq.addEventListener("change", on);
    return () => mq.removeEventListener("change", on);
  }, []);
  return reduced;
}

export { useThemeRevision } from "./useThemeRevision";

/**
 * Mounted once per window: keeps `:root` in sync with the active theme.
 * `modeOverride` is the effective Performance Mode of a secondary window that
 * does not run the perf runtime itself (robot windows follow the main
 * window's broadcast effective mode, not the user's raw choice).
 */
export function useThemeEngine(ready: boolean, modeOverride?: PerformanceMode): void {
  const theme = useThemeStore(activeTheme);
  const settingsMode = useSettingsStore((s) => s.settings?.performanceMode);
  const configMode = useThemeStore((s) => s.config?.performanceMode);
  // The perf store's effective mode (battery / frame-budget auto-switching)
  // wins once it follows the settings; before the first settings load fall
  // back to the stored choice.
  const perfMode = usePerfStore((s) => s.mode);
  const perfReady = useSettingsStore((s) => s.settings !== null);
  const performanceMode: PerformanceMode = modeOverride ?? (perfReady ? perfMode : (settingsMode ?? configMode ?? "Balanced"));
  const reducedMotion = useReducedMotion();

  useLayoutEffect(() => {
    try {
      const visuals = applyTheme(theme, document.documentElement, { performanceMode, reducedMotion });
      useVisualsStore.setState({ visuals, performanceMode });
      if (useAppStore.getState().themeId !== theme.id) useAppStore.getState().setThemeId(theme.id);
    } catch (e) {
      // The core validated it; a failure here means a client/core version skew — keep the last theme.
      console.error("failed to apply theme", e);
    }
  }, [theme, performanceMode, reducedMotion]);

  useEffect(() => (ready ? useThemeStore.getState().init() : undefined), [ready]);
}

function accentFallback(): string[] {
  const css = getComputedStyle(document.documentElement);
  return ["--ok-color-violet", "--ok-color-accent", "--ok-color-magenta"].map((v) => css.getPropertyValue(v).trim());
}

/** Scene parameters for a theme's background under the effective visuals. */
export function sceneParams(visuals: EffectiveVisuals): SceneParams {
  const bg = visuals.background;
  return {
    kind: bg.kind,
    colors: scenePalette(bg.colors, accentFallback()),
    intensity: bg.intensity,
    speed: bg.speed,
    density: bg.density,
    fps: bg.animated ? bg.fps : 0,
    resolution: bg.resolution,
  };
}

/**
 * A procedural background canvas. `webgl` loads the shader chunk lazily and
 * falls back to the 2D scene if WebGL is unavailable.
 */
export function BackgroundCanvas({ visuals, className }: { visuals: EffectiveVisuals; className?: string }) {
  const ref = useRef<HTMLCanvasElement>(null);
  const handle = useRef<SceneHandle | null>(null);
  const renderer = visuals.background.renderer;
  const params = sceneParams(visuals);
  const key = JSON.stringify(params);

  useEffect(() => {
    const canvas = ref.current;
    if (!canvas || renderer === "css") return;
    let disposed = false;
    if (renderer === "webgl") {
      void import("./backgrounds/webglNebula").then(({ runWebglNebula }) => {
        if (disposed) return;
        handle.current = runWebglNebula(canvas, sceneParams(useVisualsStore.getState().visuals ?? visuals)) ?? runScene(canvas, params);
      });
    } else {
      handle.current = runScene(canvas, params);
    }
    return () => {
      disposed = true;
      handle.current?.dispose();
      handle.current = null;
    };
    // A renderer switch needs a fresh canvas context; parameter changes go through update().
  }, [renderer]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    handle.current?.update(params);
  }, [key]); // eslint-disable-line react-hooks/exhaustive-deps

  if (renderer === "css") return null;
  // `key` on the element: WebGL and 2D contexts cannot share a canvas.
  return <canvas key={renderer} ref={ref} className={className} aria-hidden="true" />;
}

/** The app background (replaces the static `.app__backdrop`). */
export function ThemeBackdrop() {
  const applied = useVisualsStore((s) => s.visuals);
  // Hidden / minimized window: freeze the scene (one static frame).
  const paused = usePerfStore((s) => s.policy.paused);
  const visuals = applied && paused ? { ...applied, background: { ...applied.background, animated: false } } : applied;
  return (
    <div
      className={visuals && visuals.background.renderer !== "css" ? "app__backdrop app__backdrop--canvas" : "app__backdrop"}
      aria-hidden="true"
    >
      {visuals && <BackgroundCanvas visuals={visuals} className="ok-backdrop__canvas" />}
    </div>
  );
}

/** Effective visuals of an arbitrary theme under the current policy (editor previews). */
export function useEffectiveVisuals(theme: ThemeConfig): EffectiveVisuals {
  const performanceMode = useVisualsStore((s) => s.performanceMode);
  const reducedMotion = useReducedMotion();
  return effectiveVisuals(theme, { performanceMode, reducedMotion });
}
