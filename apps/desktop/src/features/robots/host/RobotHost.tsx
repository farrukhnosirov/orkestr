/**
 * Main-window robot host (ADR-0016). Tracks every agent of every project
 * over RPC events, decides which robots are on the desktop (max visible,
 * linger after completion, user-hidden, auto-hide idle) and opens / closes
 * the robot windows through the shell. It also answers robot requests:
 * click → Agents panel at that agent, hide, assign (context menu or drop on
 * the ProjectRail), voice on/off. Renders only the assign confirmation.
 */
import type { Event, RobotsConfig } from "@orkestr/protocol";
import { topic } from "@orkestr/protocol";
import { useEffect, useMemo, useRef, useState } from "react";

import { workspaceActions } from "../../../components/workspaceActions";
import { maybeClient } from "../../../core/client";
import { useAppStore } from "../../../store/app";
import { collectDropZones } from "../model/layout";
import {
  applyRobotEvent,
  diffWindows,
  mergeRobotEntries,
  nextExpiry,
  pruneSlots,
  ROBOT_AGENT_EVENTS,
  type RobotSlots,
  selectRobots,
} from "../model/visibility";
import { clusterOpenArgs, type OpenWindow, robotOpenArgs, slotDesign, staleWindows } from "../model/windows";
import { type PositionSaved, ROBOT_EVENTS, type RobotDrop, robotShell } from "../shell";
import { useRobotsStore } from "../store";
import { AssignDialog } from "./AssignDialog";
import { focusAgent } from "./focus";

const EMPTY: RobotSlots = new Map();

async function fetchEntries(agentIds: string[] | null) {
  const c = maybeClient();
  if (!c) return [];
  const params = agentIds ? { agentIds, includeFinished: true } : {};
  return (await c.call("robots.list", params)).robots;
}

export function RobotHost() {
  const connected = useAppStore((s) => s.connection.state === "open");
  const projectIds = useAppStore((s) => s.projects.map((p) => p.id).join(","));
  const config = useRobotsStore((s) => s.config);
  const hidden = useRobotsStore((s) => s.hidden);
  const [slots, setSlots] = useState<RobotSlots>(EMPTY);
  const [now, setNow] = useState(() => Date.now());
  const [assign, setAssign] = useState<RobotDrop | null>(null);
  const slotsRef = useRef(slots);
  slotsRef.current = slots;
  const configRef = useRef<RobotsConfig | null>(config);
  configRef.current = config;
  const open = useRef(new Map<string, OpenWindow>());
  const clusterOpen = useRef(false);
  const overflowRef = useRef<string[]>([]);

  // The robots document.
  useEffect(() => {
    if (connected) void useRobotsStore.getState().load();
  }, [connected]);

  // Active agents (+ fresh designs whenever the designs change; positions do not matter here).
  const designsKey = config ? JSON.stringify([config.defaultDesign, config.designs]) : null;
  useEffect(() => {
    if (!connected || designsKey === null) return undefined;
    let cancelled = false;
    const lingering = [...slotsRef.current.values()].filter((s) => s.finishedAt !== null).map((s) => s.agentId);
    Promise.all([fetchEntries(null), lingering.length ? fetchEntries(lingering) : Promise.resolve([])])
      .then(([active, rest]) => {
        if (!cancelled) setSlots((prev) => mergeRobotEntries(prev, [...rest, ...active], Date.now()));
      })
      .catch((e: unknown) => console.warn("robots.list failed", e));
    return () => {
      cancelled = true;
    };
  }, [connected, designsKey]);

  // Live agent events of every project.
  useEffect(() => {
    if (!connected || !projectIds) return undefined;
    const c = maybeClient();
    if (!c) return undefined;
    const topics = projectIds.split(",").map((id) => topic.project(id));
    return c.subscribe(topics, (ev: Event) => {
      if (!ROBOT_AGENT_EVENTS.has(ev.kind)) return;
      setSlots((prev) => applyRobotEvent(prev, ev, Date.now()));
      if (ev.kind === "AgentSpawned") {
        fetchEntries([ev.payload.agent.id])
          .then((entries) => setSlots((prev) => mergeRobotEntries(prev, entries, Date.now())))
          .catch((e: unknown) => console.warn("robots.list failed", e));
      }
    });
  }, [connected, projectIds]);

  // Expire lingering robots exactly when their time is up.
  const lingerMs = (config?.lingerSeconds ?? 90) * 1000;
  useEffect(() => {
    const at = nextExpiry(slots, lingerMs);
    if (at === null) return undefined;
    const timer = window.setTimeout(
      () => {
        const t = Date.now();
        setNow(t);
        setSlots((prev) => pruneSlots(prev, t, lingerMs));
      },
      Math.max(50, at - Date.now() + 20),
    );
    return () => window.clearTimeout(timer);
  }, [slots, lingerMs]);

  const selection = useMemo(
    () =>
      selectRobots(
        slots,
        { enabled: config?.enabled ?? false, maxVisible: config?.maxVisible ?? 6, lingerMs, hidden },
        Math.max(now, Date.now()),
      ),
    [slots, config, lingerMs, hidden, now],
  );
  overflowRef.current = selection.overflow;

  // Windows follow the selection.
  useEffect(() => {
    if (!robotShell.available() || !config) return;
    const stale = staleWindows(open.current, slots, config);
    for (const id of stale) {
      open.current.delete(id);
      void robotShell.close(id).catch((e: unknown) => console.warn("robot_close", e));
    }
    const { toOpen, toClose } = diffWindows(new Set(open.current.keys()), selection.visible);
    for (const id of toClose) {
      open.current.delete(id);
      void robotShell.close(id).catch((e: unknown) => console.warn("robot_close", e));
    }
    const taken = new Set([...open.current.values()].flatMap((w) => (w.positionKey ? [w.positionKey] : [])));
    for (const id of toOpen) {
      const slot = slots.get(id);
      if (!slot) continue;
      const { args, window: win } = robotOpenArgs(slot, config, selection.visible.indexOf(id), taken);
      if (win.positionKey) taken.add(win.positionKey);
      open.current.set(id, win);
      void robotShell.open(args).catch((e: unknown) => {
        console.warn("robot_open", e);
        open.current.delete(id);
      });
    }
    if (selection.overflow.length > 0) {
      if (!clusterOpen.current) {
        clusterOpen.current = true;
        void robotShell.open(clusterOpenArgs(config, selection.visible.length)).catch((e: unknown) => {
          console.warn("robot_open cluster", e);
          clusterOpen.current = false;
        });
      }
      void robotShell.sendCluster(selection.overflow).catch(() => undefined);
    } else if (clusterOpen.current) {
      clusterOpen.current = false;
      void robotShell.close(null).catch(() => undefined);
    }
  }, [selection, slots, config]);

  // Start from a clean desktop (windows left over from a reloaded main window
  // would otherwise be orphaned) and close every robot when the host goes away.
  useEffect(() => {
    void robotShell.closeAll().catch(() => undefined);
    return () => {
      open.current.clear();
      clusterOpen.current = false;
      void robotShell.closeAll().catch(() => undefined);
    },
    [],
  );

  // Drop zones = project tiles in the rail (re-measured while robots are out).
  const anyOut = selection.visible.length > 0;
  useEffect(() => {
    if (!anyOut || !robotShell.available()) return undefined;
    const report = () => void robotShell.setDropZones(collectDropZones(document)).catch(() => undefined);
    report();
    const timer = window.setInterval(report, 2000);
    window.addEventListener("resize", report);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("resize", report);
    };
  }, [anyOut, projectIds]);

  // Requests from robot windows.
  useEffect(() => {
    const subs = [
      robotShell.on<{ agentId: string }>(ROBOT_EVENTS.openAgent, (p) => {
        void focusAgent(p.agentId, slotsRef.current.get(p.agentId)?.projectId);
      }),
      robotShell.on<{ agentId: string }>(ROBOT_EVENTS.hide, (p) => useRobotsStore.getState().hide(p.agentId)),
      robotShell.on<RobotDrop>(ROBOT_EVENTS.assign, (p) => setAssign(p)),
      robotShell.on<RobotDrop>(ROBOT_EVENTS.drop, (p) => setAssign(p)),
      robotShell.on(ROBOT_EVENTS.openCluster, () => workspaceActions.openPanel("agents")),
      robotShell.on<PositionSaved>(ROBOT_EVENTS.positionSaved, (p) => useRobotsStore.getState().notePosition(p.key, p.position)),
      robotShell.on(ROBOT_EVENTS.clusterReady, () => {
        void robotShell.sendCluster(overflowRef.current).catch(() => undefined);
      }),
      robotShell.on<{ agentId: string; enabled: boolean }>(ROBOT_EVENTS.voice, (p) => {
        const cfg = configRef.current;
        const slot = slotsRef.current.get(p.agentId);
        if (!cfg || !slot) return;
        const design = slotDesign(slot, cfg);
        const next = { ...design, voice: { ...design.voice, enabled: p.enabled } };
        // INTEGRATOR(voice-sound): the voice engine reads `design.voice` (enabled + voiceId) per robot.
        void useRobotsStore
          .getState()
          .saveDesign(slot.templateId ? { kind: "template", templateId: slot.templateId } : { kind: "default" }, next);
      }),
    ];
    return () => subs.forEach((s) => void s.then((off) => off()));
  }, []);

  if (!assign) return null;
  const slot = slots.get(assign.agentId) ?? null;
  return <AssignDialog request={assign} slot={slot} onClose={() => setAssign(null)} />;
}
