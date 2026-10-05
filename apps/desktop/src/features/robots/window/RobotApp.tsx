/**
 * Root of the `robot.html` entry: routes to one robot, the overflow cluster,
 * or (no core needed) a preview robot cycling its states for development.
 */
import type { PerformanceMode } from "@orkestr/protocol";
import { MotionConfig } from "motion/react";
import { useEffect, useState } from "react";

import { useT } from "../../../i18n";
import type { BubbleSpec } from "../model/bubble";
import { defaultDesign } from "../model/design";
import { robotWindowLayout } from "../model/layout";
import { parseRobotRoute } from "../model/route";
import { previewStateAt, stateColor } from "../model/states";
import { accentOf } from "../model/design";
import { useReducedMotion } from "../render/hooks";
import { RobotView } from "../render/RobotView";
import { ClusterWindow } from "./ClusterWindow";
import { useRobotEnv } from "./env";
import { RobotWindow } from "./RobotWindow";
import { useThemeEngine } from "../../theme/ThemeRuntime";
import { SpeechBubble } from "./SpeechBubble";

const PREVIEW_BUBBLES: Partial<Record<ReturnType<typeof previewStateAt>, BubbleSpec>> = {
  Completed: { key: "testsPassed", values: { count: 42 }, tone: "success", ttlMs: 0, important: true, chatty: false },
  Error: { key: "buildFailed", values: {}, tone: "danger", ttlMs: 0, important: true, chatty: false },
  Waiting: { key: "approval", values: {}, tone: "warning", ttlMs: 0, important: true, chatty: false },
};

function PreviewRobot({ mode }: { mode: PerformanceMode }) {
  const [t0] = useState(() => performance.now());
  const [now, setNow] = useState(t0);
  const reducedMotion = useReducedMotion();
  useEffect(() => {
    const timer = window.setInterval(() => setNow(performance.now()), 250);
    return () => window.clearInterval(timer);
  }, []);
  const design = defaultDesign();
  const layout = robotWindowLayout(design.scale);
  const state = previewStateAt(now - t0);
  const bubble = PREVIEW_BUBBLES[state] ?? null;
  return (
    <div className="robot-window" style={{ width: layout.width, height: layout.height, ["--robot-color" as string]: stateColor(state, accentOf(design)) }}>
      <SpeechBubble bubble={bubble} nonce={0} reducedMotion={reducedMotion} />
      <div className="robot-window__robot" style={{ left: layout.robot.x, top: layout.robot.y, width: layout.robot.w, height: layout.robot.h }}>
        <span className="robot-window__halo" aria-hidden="true" />
        <RobotView design={design} state={state} size={layout.robot.w} mode={mode} hovered />
      </div>
    </div>
  );
}

export function RobotApp() {
  const { t } = useT("robots");
  const [route] = useState(() => parseRobotRoute(window.location.hash));
  const env = useRobotEnv();
  useThemeEngine(env.ready);
  const reducedMotion = useReducedMotion();

  let content;
  if (route.kind === "preview") content = <PreviewRobot mode="UltraVisual" />;
  else if (route.kind === "invalid") content = <div className="robot-error">{t("robot.invalidRoute")}</div>;
  else if (env.error) content = <div className="robot-error" title={env.error}>{t("robot.offline")}</div>;
  else if (!env.ready) content = null;
  else if (route.kind === "cluster") content = <ClusterWindow mode={env.mode} />;
  else content = <RobotWindow agentId={route.agentId} mode={env.mode} />;

  return <MotionConfig reducedMotion={reducedMotion ? "always" : "never"}>{content}</MotionConfig>;
}
