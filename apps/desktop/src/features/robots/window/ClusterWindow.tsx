/**
 * The overflow cluster (window `robot-cluster`): robots beyond `maxVisible`
 * collapse into one glass pill with their faces stacked and a count. Click →
 * Agents panel in the main window; drag to move like a robot.
 */
import type { AgentInstance, PerformanceMode, RobotDesign } from "@orkestr/protocol";
import { type PointerEvent as ReactPointerEvent, useEffect, useRef, useState } from "react";

import { useT } from "../../../i18n";
import { CLUSTER_SIZE, isDragGesture } from "../model/layout";
import { visualStateFor } from "../model/states";
import { CLUSTER_POSITION_KEY } from "../model/windows";
import { RobotView } from "../render/RobotView";
import { ROBOT_EVENTS, type RobotMoved, robotShell } from "../shell";
import { robotClient } from "./env";

interface Face {
  agent: AgentInstance;
  design: RobotDesign;
}

const FACES = 3;

export function ClusterWindow({ mode }: { mode: PerformanceMode }) {
  const { t } = useT("robots");
  const [ids, setIds] = useState<string[]>([]);
  const [faces, setFaces] = useState<Face[]>([]);
  const press = useRef<{ x: number; y: number; dragged: boolean } | null>(null);

  useEffect(() => {
    const subs = [
      robotShell.on<string[]>(ROBOT_EVENTS.cluster, setIds),
      robotShell.on<RobotMoved>(ROBOT_EVENTS.moved, (p) => {
        const position = { monitor: p.monitor, x: p.x, y: p.y };
        robotClient()
          ?.call("robots.setConfig", { position: { key: CLUSTER_POSITION_KEY, position } })
          .then(() => robotShell.positionSaved({ key: CLUSTER_POSITION_KEY, position }))
          .catch((e: unknown) => console.warn("saving cluster position failed", e));
      }),
    ];
    void robotShell.toMain("cluster-ready", {}).catch(() => undefined);
    const shape = { ellipses: [], rects: [{ x: 0, y: 0, w: CLUSTER_SIZE.width, h: CLUSTER_SIZE.height }] };
    void robotShell.setShape(shape, false, true).catch(() => undefined);
    return () => subs.forEach((s) => void s.then((off) => off()));
  }, []);

  const key = ids.slice(0, FACES).join(",");
  useEffect(() => {
    const c = robotClient();
    if (!c || !key) {
      setFaces([]);
      return;
    }
    c.call("robots.list", { agentIds: key.split(","), includeFinished: true })
      .then(({ robots }) => setFaces(robots.map((r) => ({ agent: r.agent, design: r.design }))))
      .catch((e: unknown) => console.warn("robots.list failed", e));
  }, [key]);

  const open = () => void robotShell.toMain("open-cluster", {}).catch(() => undefined);
  const onPointerDown = (e: ReactPointerEvent) => {
    if (e.button === 0) press.current = { x: e.clientX, y: e.clientY, dragged: false };
  };
  const onPointerMove = (e: ReactPointerEvent) => {
    const p = press.current;
    if (!p || p.dragged || !isDragGesture(e.clientX - p.x, e.clientY - p.y)) return;
    p.dragged = true;
    void robotShell.startDragging().catch(() => undefined);
  };
  const onPointerUp = () => {
    const p = press.current;
    press.current = null;
    if (p && !p.dragged) open();
  };

  return (
    <div
      className="robot-cluster"
      style={{ width: CLUSTER_SIZE.width, height: CLUSTER_SIZE.height }}
      role="button"
      tabIndex={0}
      aria-label={t("cluster.label", { count: ids.length })}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onKeyDown={(e) => (e.key === "Enter" || e.key === " ") && open()}
    >
      <div className="robot-cluster__faces" aria-hidden="true">
        {faces.map((f, i) => {
          const v = visualStateFor(f.agent.state);
          return (
            <div key={f.agent.id} className="robot-cluster__face" style={{ zIndex: FACES - i }}>
              <RobotView design={{ ...f.design, mode: "TwoD" }} state={v.state} asleep={v.asleep} size={52} mode={mode === "UltraVisual" ? "Balanced" : mode} />
            </div>
          );
        })}
      </div>
      <div className="robot-cluster__text">
        <span className="robot-cluster__count">{t("cluster.count", { count: ids.length })}</span>
        <span className="robot-cluster__hint">{t("cluster.hint")}</span>
      </div>
    </div>
  );
}
