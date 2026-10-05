/**
 * One floating robot (window `robot-<agentId>`, ADR-0016). Shows the agent's
 * real state (`agent:<id>` events), comments on results in a speech bubble,
 * reports its interactive shape to the shell (click-through outside it),
 * drags with the OS (`startDragging`), saves where it was put, and offers the
 * context menu. Clicking the robot or its bubble opens the agent in the main
 * window (Scenario H: QA robot completes → celebration → bubble → click →
 * QA result).
 */
import type { AgentInstance, Event, PerformanceMode, RobotDesign } from "@orkestr/protocol";
import { topic } from "@orkestr/protocol";
import { clsx } from "clsx";
import { type PointerEvent as ReactPointerEvent, useCallback, useEffect, useMemo, useRef, useState } from "react";

import { useT } from "../../../i18n";
import { useAppStore } from "../../../store/app";
import { bubbleFor, type BubbleSpec, initialBubble, mayReplace } from "../model/bubble";
import { defaultDesign } from "../model/design";
import { type Box, hitShapeFor, isDragGesture, robotWindowLayout } from "../model/layout";
import { isRunning, robotMenu, type RobotMenuItemId } from "../model/menu";
import { stateColor, visualStateFor } from "../model/states";
import { accentOf } from "../model/design";
import { useReducedMotion } from "../render/hooks";
import { RobotView } from "../render/RobotView";
import { ROBOT_EVENTS, type RobotMoved, robotShell } from "../shell";
import { robotClient } from "./env";
import { RobotMenu } from "./RobotMenu";
import { SpeechBubble } from "./SpeechBubble";

interface Shown {
  spec: BubbleSpec;
  shownAt: number;
  nonce: number;
}

function applyAgent(agent: AgentInstance | null, ev: Event): AgentInstance | null {
  if (!agent) return agent;
  switch (ev.kind) {
    case "AgentUpdated":
    case "AgentSpawned":
      return ev.payload.agent.id === agent.id ? ev.payload.agent : agent;
    case "AgentStateChanged":
      return ev.payload.agentId === agent.id
        ? { ...agent, state: ev.payload.to, activity: ev.payload.activity ?? agent.activity }
        : agent;
    case "AgentStopped":
      return ev.payload.agentId === agent.id ? { ...agent, stoppedAt: agent.stoppedAt ?? ev.ts } : agent;
    default:
      return agent;
  }
}

export function RobotWindow({ agentId, mode }: { agentId: string; mode: PerformanceMode }) {
  const { t } = useT(["robots", "common"]);
  const projects = useAppStore((s) => s.projects);
  const [agent, setAgent] = useState<AgentInstance | null>(null);
  const [design, setDesign] = useState<RobotDesign>(defaultDesign);
  const [hovered, setHovered] = useState(false);
  const [dragging, setDragging] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const [shown, setShown] = useState<Shown | null>(null);
  const [bubbleBox, setBubbleBox] = useState<Box | null>(null);
  const bubbleRef = useRef<HTMLDivElement | null>(null);
  const press = useRef<{ x: number; y: number; dragged: boolean } | null>(null);
  const announced = useRef(false);
  const designRef = useRef(design);
  designRef.current = design;
  const reducedMotion = useReducedMotion();

  // Agent + resolved design; refetched when the main window changes the robots document.
  const load = useCallback(() => {
    const c = robotClient();
    if (!c) return;
    c.call("robots.list", { agentIds: [agentId], includeFinished: true })
      .then(({ robots }) => {
        const entry = robots[0];
        if (!entry) return;
        setAgent(entry.agent);
        setDesign(entry.design);
        if (!announced.current) {
          announced.current = true;
          const spec = initialBubble(entry.agent.state, entry.agent.activity, entry.design.behaviour.bubble);
          if (spec) setShown((cur) => cur ?? { spec, shownAt: Date.now(), nonce: 1 });
        }
      })
      .catch((e: unknown) => console.warn("robots.list failed", e));
  }, [agentId]);

  useEffect(() => {
    load();
    const off = robotShell.on(ROBOT_EVENTS.configChanged, load);
    return () => void off.then((f) => f());
  }, [load]);

  // Live agent events → state + bubbles.
  useEffect(() => {
    const c = robotClient();
    if (!c) return undefined;
    return c.subscribe([topic.agent(agentId)], (ev: Event) => {
      setAgent((prev) => applyAgent(prev, ev));
      const spec = bubbleFor(ev, designRef.current.behaviour.bubble);
      if (!spec) return;
      setShown((cur) => {
        const now = Date.now();
        return mayReplace(cur, spec, now) ? { spec, shownAt: now, nonce: (cur?.nonce ?? 0) + 1 } : cur;
      });
    });
  }, [agentId]);

  // Bubble auto-hide.
  useEffect(() => {
    if (!shown || shown.spec.ttlMs <= 0) return undefined;
    const timer = window.setTimeout(() => setShown((cur) => (cur === shown ? null : cur)), shown.spec.ttlMs);
    return () => window.clearTimeout(timer);
  }, [shown]);

  // Shell signals: hover (cursor over the interactive shape), drag, settled position.
  useEffect(() => {
    const subs = [
      robotShell.on<boolean>(ROBOT_EVENTS.hover, setHovered),
      robotShell.on<boolean>(ROBOT_EVENTS.drag, setDragging),
      robotShell.on<RobotMoved>(ROBOT_EVENTS.moved, (p) => {
        const key = agent?.templateId;
        const c = robotClient();
        if (!key || !c) return;
        const position = { monitor: p.monitor, x: p.x, y: p.y };
        c.call("robots.setConfig", { position: { key, position } })
          .then(() => robotShell.positionSaved({ key, position }))
          .catch((e: unknown) => console.warn("saving robot position failed", e));
      }),
    ];
    return () => subs.forEach((s) => void s.then((off) => off()));
  }, [agent?.templateId]);

  const layout = useMemo(() => robotWindowLayout(design.scale), [design.scale]);
  const visual = visualStateFor(agent?.state ?? "Idle");
  const passThrough = design.behaviour.clickThroughIdle && (visual.state === "Idle" || visual.asleep) && !shown && !menuOpen;

  // Measure the bubble after it renders (it animates in; re-measure once settled).
  useEffect(() => {
    if (!shown) {
      setBubbleBox(null);
      return undefined;
    }
    const measure = () => {
      const r = bubbleRef.current?.getBoundingClientRect();
      setBubbleBox(r && r.width > 0 ? { x: r.left, y: r.top, w: r.width, h: r.height } : null);
    };
    const raf = requestAnimationFrame(measure);
    const timer = window.setTimeout(measure, 400);
    return () => {
      cancelAnimationFrame(raf);
      window.clearTimeout(timer);
    };
  }, [shown]);

  // Report the interactive shape to the shell's cursor poller.
  useEffect(() => {
    const shape = hitShapeFor({ layout, bubble: bubbleBox, menuOpen });
    robotShell.setShape(shape, passThrough, design.behaviour.snap).catch((e: unknown) => console.warn("robot_set_shape", e));
  }, [layout, bubbleBox, menuOpen, passThrough, design.behaviour.snap]);

  const openAgent = () => void robotShell.toMain("open-agent", { agentId }).catch((e: unknown) => console.warn(e));

  const onPointerDown = (e: ReactPointerEvent) => {
    if (e.button !== 0) return;
    press.current = { x: e.clientX, y: e.clientY, dragged: false };
  };
  const onPointerMove = (e: ReactPointerEvent) => {
    const p = press.current;
    if (!p || p.dragged || !isDragGesture(e.clientX - p.x, e.clientY - p.y)) return;
    p.dragged = true;
    void robotShell.startDragging().catch((err: unknown) => console.warn("startDragging", err));
  };
  const onPointerUp = () => {
    const p = press.current;
    press.current = null;
    if (p && !p.dragged && !menuOpen) openAgent();
  };

  const onAction = (id: RobotMenuItemId) => {
    setMenuOpen(false);
    switch (id) {
      case "open":
        openAgent();
        break;
      case "hide":
        void robotShell.toMain("hide", { agentId });
        break;
      case "voice":
        void robotShell.toMain("voice", { agentId, enabled: !design.voice.enabled });
        break;
      case "stop":
        robotClient()
          ?.call("agents.stop", { agentId })
          .catch((e: unknown) => console.warn("agents.stop failed", e));
        break;
      case "assign":
        break;
    }
  };

  const assignable = projects.filter((p) => p.id !== agent?.projectId);
  const items = robotMenu({
    state: agent?.state ?? "Idle",
    stoppedAt: agent?.stoppedAt ?? null,
    voiceEnabled: design.voice.enabled,
    assignableProjects: assignable.length,
    hasTemplate: Boolean(agent?.templateId),
  });
  const accent = accentOf(design);
  const color = stateColor(visual.state, accent);
  const name = agent?.name ?? t("robot.unknownAgent");
  const stateLabel = visual.asleep ? t("common:status.agent.Stopped") : t(`common:status.agent.${visual.state}`);

  return (
    <div
      className={clsx(
        "robot-window",
        hovered && "robot-window--hover",
        dragging && "robot-window--drag",
        (mode === "Performance" || mode === "BatterySaver") && "robot-window--lite",
      )}
      style={{ width: layout.width, height: layout.height, ["--robot-color" as string]: color }}
    >
      <SpeechBubble ref={bubbleRef} bubble={shown?.spec ?? null} nonce={shown?.nonce ?? 0} reducedMotion={reducedMotion} onClick={openAgent} />
      <div
        className="robot-window__robot"
        style={{ left: layout.robot.x, top: layout.robot.y, width: layout.robot.w, height: layout.robot.h }}
        role="button"
        tabIndex={0}
        aria-label={t("robot.label", { name, state: stateLabel })}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") openAgent();
          if (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) setMenuOpen(true);
        }}
        onContextMenu={(e) => {
          e.preventDefault();
          setMenuOpen(true);
        }}
      >
        <span className="robot-window__halo" aria-hidden="true" />
        <RobotView design={design} state={visual.state} asleep={visual.asleep} size={layout.robot.w} mode={mode} hovered={hovered || dragging} />
        <span className="robot-window__tag" aria-hidden="true">
          <span className="robot-window__tag-dot" />
          {name}
          {isRunning(agent?.state ?? "Idle", agent?.stoppedAt ?? null) && agent?.activity ? ` · ${agent.activity}` : ""}
        </span>
      </div>
      {menuOpen && (
        <RobotMenu
          items={items}
          projects={assignable}
          onAction={onAction}
          onAssign={(projectId) => {
            setMenuOpen(false);
            void robotShell.toMain("assign", { agentId, projectId });
          }}
          onClose={() => setMenuOpen(false)}
        />
      )}
    </div>
  );
}
