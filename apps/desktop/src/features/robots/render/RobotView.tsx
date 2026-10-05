/**
 * A robot that animates itself: picks the renderer (2D SVG, or the lazy 3D
 * one in Ultra Visual), the frame rate tier (§31) and drives the pose clock.
 */
import type { PerformanceMode, RobotDesign } from "@orkestr/protocol";
import { lazy, Suspense, useMemo } from "react";

import { robotEffects, robotFps } from "../model/perf";
import { poseAt, STATIC_FRAME_MS } from "../model/pose";
import type { RobotState } from "../model/states";
import { usePageHidden, useReducedMotion, useRobotClock, webglAvailable } from "./hooks";
import { RobotFigure } from "./RobotFigure";

const Robot3D = lazy(() => import("./Robot3D"));

export interface RobotViewProps {
  design: RobotDesign;
  state: RobotState;
  asleep?: boolean;
  size: number;
  mode: PerformanceMode;
  hovered?: boolean;
  className?: string;
}

let webgl: boolean | null = null;

export function RobotView({ design, state, asleep = false, size, mode, hovered = false, className }: RobotViewProps) {
  const reducedMotion = useReducedMotion();
  const hidden = usePageHidden();
  const fps = robotFps({ mode, reducedMotion, state, asleep, hovered, intensity: design.animationIntensity, hidden });
  const effects = robotEffects(mode, reducedMotion);
  const t = useRobotClock(fps, STATIC_FRAME_MS[state]);
  const pose = useMemo(() => poseAt(state, t, design.animationIntensity, asleep), [state, t, design.animationIntensity, asleep]);
  const figure = (
    <RobotFigure
      className={className}
      design={design}
      state={state}
      pose={pose}
      asleep={asleep}
      glow={effects.glow}
      particles={effects.particles}
      size={size}
      hovered={hovered}
    />
  );
  if (design.mode === "ThreeD" && effects.threeD) {
    webgl ??= webglAvailable();
    if (webgl) {
      return (
        <Suspense fallback={figure}>
          <Robot3D design={design} state={state} pose={pose} size={size} />
        </Suspense>
      );
    }
  }
  return figure;
}
